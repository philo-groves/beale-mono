import type { FormEvent, JSX } from 'react';
import { FolderPlus, GitBranch, Power, PowerOff, Trash2 } from 'lucide-react';
import type { AgentPluginRecord, AgentPluginRegistryState } from '@shared/types';
import { CenteredLoadingState } from '../../app/CenteredLoadingState';

export function PluginManagerWorkspace({
  state,
  loading,
  busy,
  error,
  repositoryUrl,
  onRepositoryUrlChange,
  onAddFilesystem,
  onAddRepository,
  onSetEnabled,
  onRemove
}: {
  state: AgentPluginRegistryState | null;
  loading: boolean;
  busy: boolean;
  error: string | null;
  repositoryUrl: string;
  onRepositoryUrlChange: (value: string) => void;
  onAddFilesystem: () => void;
  onAddRepository: () => void;
  onSetEnabled: (pluginId: string, enabled: boolean) => void;
  onRemove: (pluginId: string) => void;
}): JSX.Element {
  const plugins = state?.plugins ?? [];
  const submittingDisabled = busy || loading || repositoryUrl.trim().length === 0;

  const submitRepository = (event: FormEvent<HTMLFormElement>): void => {
    event.preventDefault();
    if (!submittingDisabled) onAddRepository();
  };

  return (
    <section className="plugin-manager-workspace" aria-label="Plugins" aria-busy={loading}>
      <div className="plugin-manager-body">
        <section className="plugin-manager-add">
          <button type="button" className="plugin-manager-file-button" disabled={busy || loading} onClick={onAddFilesystem}>
            <FolderPlus size={15} />
            <span>Add from Filesystem</span>
          </button>
          <form className="plugin-manager-repository-form" onSubmit={submitRepository}>
            <GitBranch size={15} aria-hidden="true" />
            <input
              type="url"
              value={repositoryUrl}
              placeholder="https://github.com/owner/plugin"
              disabled={busy || loading}
              onChange={(event) => onRepositoryUrlChange(event.target.value)}
            />
            <button type="submit" className="primary-button" disabled={submittingDisabled}>
              Add Repository
            </button>
          </form>
        </section>
        <header className="resource-workspace-heading">
          <h1>Plugins</h1>
          <p>Manage the plugins available to Beale agents.</p>
        </header>

        {error ? <div className="plugin-manager-error">{error}</div> : null}

        <section className="plugin-manager-catalog" aria-label="Installed plugins">
          <div className="plugin-manager-list">
            {loading ? (
              <CenteredLoadingState label="Loading plugins…" />
            ) : plugins.length > 0 ? (
              plugins.map((plugin) => (
                <PluginRow
                  key={plugin.id}
                  plugin={plugin}
                  busy={busy}
                  onSetEnabled={onSetEnabled}
                  onRemove={onRemove}
                />
              ))
            ) : (
              <div className="plugin-manager-empty">
                <strong>No plugins installed</strong>
                <span>Add an Agent Plugin directory or repository.</span>
              </div>
            )}
          </div>
        </section>
      </div>
    </section>
  );
}

function PluginRow({
  plugin,
  busy,
  onSetEnabled,
  onRemove
}: {
  plugin: AgentPluginRecord;
  busy: boolean;
  onSetEnabled: (pluginId: string, enabled: boolean) => void;
  onRemove: (pluginId: string) => void;
}): JSX.Element {
  const invalid = plugin.status === 'invalid';
  const messages = [
    ...plugin.errors,
    ...plugin.warnings,
    ...plugin.mcpServers.flatMap((server) => server.errors.map((message) => `${server.name}: ${message}`))
  ];
  const statusLabel = invalid ? 'Invalid' : plugin.enabled ? 'Enabled' : 'Disabled';
  const statusDetail = [statusLabel, plugin.version, ...messages].filter(Boolean).join(' · ');

  return (
    <article className={`plugin-manager-row ${invalid ? 'invalid' : ''}`}>
      <span className="plugin-manager-row-copy">
        <strong>{plugin.name}</strong>
        <small title={statusDetail}>{statusDetail}</small>
      </span>
      <div className="plugin-manager-actions">
        <button
          type="button"
          title={plugin.enabled ? 'Disable plugin' : 'Enable plugin'}
          disabled={busy || invalid}
          onClick={() => onSetEnabled(plugin.id, !plugin.enabled)}
        >
          {plugin.enabled ? <PowerOff size={14} /> : <Power size={14} />}
          <span>{plugin.enabled ? 'Disable' : 'Enable'}</span>
        </button>
        {plugin.source.kind !== 'builtin' ? (
          <button type="button" title="Remove plugin" disabled={busy} onClick={() => onRemove(plugin.id)}>
            <Trash2 size={14} />
            <span>Remove</span>
          </button>
        ) : null}
      </div>
    </article>
  );
}
