import type { JSX } from 'react';
import type {
  NotificationRecord,
  ProfilingReport,
  ProfilingState,
  WorkspaceRegistryEntry
} from '@shared/types';
import { NotificationDetailModal } from '../features/notifications/Notifications';
import { ProfilingModal } from '../features/settings/ProfilingModal';
import { MissingWorkspaceDirectoryDialog } from '../features/workspaces/MissingWorkspaceDirectoryDialog';

export function AppModals({
  activeNotification,
  busy,
  profilingOpen,
  profilingState,
  lastProfilingReport,
  missingDirectoryWorkspace,
  onCloseNotification,
  onCloseProfiling,
  onFlushProfilingReport,
  onSetProfilingEnabled,
  onCloseMissingDirectory,
  onRemoveMissingDirectory,
  onSteerNotification
}: {
  activeNotification: NotificationRecord | null;
  busy: boolean;
  profilingOpen: boolean;
  profilingState: ProfilingState | null;
  lastProfilingReport: ProfilingReport | null;
  missingDirectoryWorkspace: WorkspaceRegistryEntry | null;
  onCloseNotification: () => void;
  onCloseProfiling: () => void;
  onFlushProfilingReport: () => void;
  onSetProfilingEnabled: (enabled: boolean) => Promise<void>;
  onCloseMissingDirectory: () => void;
  onRemoveMissingDirectory: () => void;
  onSteerNotification: (notification: NotificationRecord, instruction: string) => void;
}): JSX.Element {
  return (
    <>
      {profilingOpen ? (
        <ProfilingModal
          state={profilingState}
          report={lastProfilingReport}
          onClose={onCloseProfiling}
          onFlush={onFlushProfilingReport}
          onSetEnabled={onSetProfilingEnabled}
        />
      ) : null}
      {activeNotification ? (
        <NotificationDetailModal
          notification={activeNotification}
          busy={busy}
          onClose={onCloseNotification}
          onSteer={(instruction) => onSteerNotification(activeNotification, instruction)}
        />
      ) : null}
      {missingDirectoryWorkspace ? (
        <MissingWorkspaceDirectoryDialog
          busy={busy}
          workspace={missingDirectoryWorkspace}
          onClose={onCloseMissingDirectory}
          onRemove={onRemoveMissingDirectory}
        />
      ) : null}
    </>
  );
}
