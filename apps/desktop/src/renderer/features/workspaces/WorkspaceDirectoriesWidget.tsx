import { useState, type JSX } from 'react';
import type { WorkspaceDirectorySelection } from '@shared/types';
import { errorMessage } from '../../lib/errors';
import { compactUserPath } from '../../lib/paths';

interface WorkspaceDirectoryProps {
  directories: readonly string[];
  disabled?: boolean;
  lockedDirectory?: string | null;
  onAdd: (selection: WorkspaceDirectorySelection) => Promise<void> | void;
  onRemove: (directory: string) => Promise<void> | void;
  onMakePrimary?: (directory: string) => Promise<void> | void;
}

export function WorkspaceDirectoriesField({ directories, disabled = false, lockedDirectory, onAdd }: WorkspaceDirectoryProps): JSX.Element {
  const [selecting, setSelecting] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const choose = (): void => {
    setSelecting(true);
    setError(null);
    void window.beale.selectWorkspaceDirectory().then(async (selection) => {
      if (!selection.canceled && selection.path) await onAdd(selection);
    }).catch((caught: unknown) => setError(errorMessage(caught))).finally(() => setSelecting(false));
  };
  return (
    <div className="settings-form-control-row workspace-overview-control-row workspace-directories-field">
      <span className="settings-form-control-copy">
        <strong>Research Directory</strong>
        <small>One dedicated directory with local Git history. Source repositories are stored separately.</small>
      </span>
      <div className="workspace-directories-field-control">
        <span title={directories[0]}>{directories[0] ? compactUserPath(directories[0]) : 'No directory selected'}</span>
        {!lockedDirectory ? <button aria-label="Choose workspace directory" disabled={disabled || selecting} onClick={choose} type="button">{directories[0] ? 'Change directory' : 'Choose directory'}</button> : null}
        {error ? <p role="alert">{error}</p> : null}
      </div>
    </div>
  );
}

export const WorkspaceDirectoriesWidget = WorkspaceDirectoriesField;
