import type { JSX } from 'react';
import { Trash2 } from 'lucide-react';
import type { WorkspaceRegistryEntry } from '@shared/types';
import { Modal } from '../../app/Modal';

export function MissingWorkspaceDirectoryDialog({
  busy,
  onClose,
  onRemove,
  workspace,
}: {
  busy: boolean;
  onClose: () => void;
  onRemove: () => void;
  workspace: Pick<WorkspaceRegistryEntry, 'workspaceName'>;
}): JSX.Element {
  return (
    <Modal
      className="missing-workspace-directory-dialog"
      closeDisabled={busy}
      footer={(
        <>
          <button disabled={busy} type="button" onClick={onClose}>Cancel</button>
          <button className="workspace-removal-action" disabled={busy} type="button" onClick={onRemove}>
            <Trash2 aria-hidden="true" size={15} />
            <span>{busy ? 'Removing…' : 'Remove Workspace'}</span>
          </button>
        </>
      )}
      onClose={onClose}
      title="Primary Directory Not Found"
    >
      <div className="missing-workspace-directory-message">
        <p>The primary directory for <strong>{workspace.workspaceName}</strong> could not be found. It may have been moved or removed.</p>
        <p>Removing the workspace from Beale will not delete any files.</p>
      </div>
    </Modal>
  );
}
