import { useEffect, useRef } from 'react';
import type { FormEvent, JSX, PointerEvent as ReactPointerEvent } from 'react';
import { FolderPlus, GitBranch, Power, PowerOff, Trash2 } from 'lucide-react';
import type { AgentPluginRecord, AgentPluginRegistryState } from '@shared/types';
import { isOptionalAgentFeaturePlugin } from '../../../shared/optionalAgentFeatures';
import { CenteredLoadingState } from '../../app/CenteredLoadingState';
import { CollectionSidebar } from '../../app/CollectionSidebar';

export function PluginsSidebar({
  state,
  selectedPluginId,
  collapsed,
  loading,
  error,
  onSelectPlugin,
  onResizePointerDown
}: {
  state: AgentPluginRegistryState | null;
  selectedPluginId: string | null;
  collapsed: boolean;
  loading: boolean;
  error: string | null;
  onSelectPlugin: (pluginId: string) => void;
  onResizePointerDown: (event: ReactPointerEvent<HTMLDivElement>) => void;
}): JSX.Element {
  const plugins = state?.plugins.filter((plugin) => !isOptionalAgentFeaturePlugin(plugin)) ?? [];
  return (
    <CollectionSidebar
      title="Plugins"
      label="Plugins sidebar"
      collapsed={collapsed}
      error={error}
      updateKey={plugins.map((plugin) => `${plugin.id}:${plugin.enabled}:${plugin.status}`).join(',')}
      onResizePointerDown={onResizePointerDown}
    >
      <nav className="collection-sidebar-list sidebar-list-scroll-content" aria-label="Installed plugins">
        <div className="collection-sidebar-list-heading">Installed</div>
        {loading ? <p className="collection-sidebar-empty">Loading plugins…</p> : plugins.length === 0 ? (
          <p className="collection-sidebar-empty">No plugins installed</p>
        ) : plugins.map((plugin) => {
          const selected = selectedPluginId === plugin.id;
          return (
            <div className={`workspace-item-row no-menu ${selected ? 'active' : ''}`.trim()} key={plugin.id}>
              <button type="button" className="workspace-item collection-sidebar-item" aria-current={selected ? 'page' : undefined} onClick={() => onSelectPlugin(plugin.id)}>
                <span className="collection-sidebar-item-copy">
                  <span>{plugin.name}</span>
                  <small>{plugin.status === 'invalid' ? 'Invalid' : plugin.enabled ? 'Enabled' : 'Disabled'}</small>
                </span>
              </button>
            </div>
          );
        })}
      </nav>
    </CollectionSidebar>
  );
}

export function PluginManagerWorkspace({
  state,
  selectedPluginId = null,
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
  selectedPluginId?: string | null;
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
  const plugins = state?.plugins.filter((plugin) => !isOptionalAgentFeaturePlugin(plugin)) ?? [];
  const listRef = useRef<HTMLDivElement | null>(null);
  const submittingDisabled = busy || loading || repositoryUrl.trim().length === 0;

  useEffect(() => {
    if (!selectedPluginId) return;
    const selectedRow = [...listRef.current?.querySelectorAll<HTMLElement>('.plugin-manager-row') ?? []]
      .find((row) => row.dataset.pluginId === selectedPluginId);
    selectedRow?.scrollIntoView({ block: 'nearest', behavior: 'smooth' });
  }, [selectedPluginId, state]);

  const submitRepository = (event: FormEvent<HTMLFormElement>): void => {
    event.preventDefault();
    if (!submittingDisabled) onAddRepository();
  };

  return (
    <section className="plugin-manager-workspace" aria-label="Plugins" aria-busy={loading}>
      <div className="plugin-manager-body wide-content-container">
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
          <div className="plugin-manager-list" ref={listRef}>
            {loading ? (
              <CenteredLoadingState label="Loading plugins…" />
            ) : plugins.length > 0 ? (
              plugins.map((plugin) => (
                <PluginRow
                  key={plugin.id}
                  plugin={plugin}
                  selected={plugin.id === selectedPluginId}
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
  selected,
  busy,
  onSetEnabled,
  onRemove
}: {
  plugin: AgentPluginRecord;
  selected: boolean;
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
    <article className={`plugin-manager-row ${invalid ? 'invalid' : ''} ${selected ? 'selected' : ''}`.trim()} data-plugin-id={plugin.id}>
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
