import { startTransition, useCallback, useEffect, useRef, useState } from 'react';
import type { Dispatch, SetStateAction } from 'react';
import type {
  HostEnvironment,
  OpenAiAccountStatus,
  WorkspaceRegistryState,
  WindowChromeState,
  WorkspaceSnapshot
} from '@shared/types';
import { devInstrumentation, recordNextFrameTiming } from '../devInstrumentation';
import { errorMessage } from '../lib/errors';
import {
  isStaleWorkspaceSnapshot,
  selectRunId,
  snapshotMetricDetail
} from '../view-models/runDetailUpdates';

export type WorkspaceStartupPhase = 'shell' | 'registry' | 'ready';

const WORKSPACE_REGISTRY_REFRESH_INTERVAL_MS = 5_000;
const WORKSPACE_MEMORY_REFRESH_INTERVAL_MS = 5_000;
const SELECTED_RUN_STORAGE_KEY = 'beale:selected-run';

function restoredSelectedRunId(): string | null {
  if (typeof window === 'undefined') return null;
  try {
    const id = window.sessionStorage.getItem(SELECTED_RUN_STORAGE_KEY);
    return id && /^[A-Za-z0-9_.:-]{1,128}$/u.test(id) ? id : null;
  } catch {
    return null;
  }
}

export function useWorkspaceRuntime(onError: (message: string) => void): {
  snapshot: WorkspaceSnapshot | null;
  workspaceRegistry: WorkspaceRegistryState | null;
  hostEnvironment: HostEnvironment | null;
  windowChromeState: WindowChromeState;
  openAiStatus: OpenAiAccountStatus | null;
  startupPhase: WorkspaceStartupPhase;
  selectedRunId: string | null;
  setWorkspaceRegistry: Dispatch<SetStateAction<WorkspaceRegistryState | null>>;
  setOpenAiStatus: Dispatch<SetStateAction<OpenAiAccountStatus | null>>;
  setSelectedRunId: Dispatch<SetStateAction<string | null>>;
  selectPendingRunId: (runId: string, workspaceId: string) => void;
  releasePendingRunId: (runId: string) => void;
  applySnapshot: (next: WorkspaceSnapshot | null, selectedRunIdOverride?: string) => void;
  loadSnapshot: () => Promise<void>;
  loadWorkspaceRegistry: () => Promise<void>;
} {
  const [snapshot, setSnapshot] = useState<WorkspaceSnapshot | null>(null);
  const [workspaceRegistry, setWorkspaceRegistry] = useState<WorkspaceRegistryState | null>(null);
  const [hostEnvironment, setHostEnvironment] = useState<HostEnvironment | null>(null);
  const [windowChromeState, setWindowChromeState] = useState<WindowChromeState>({ isMaximized: false, isFullScreen: false });
  const [openAiStatus, setOpenAiStatus] = useState<OpenAiAccountStatus | null>(null);
  const [startupPhase, setStartupPhase] = useState<WorkspaceStartupPhase>('shell');
  const [selectedRunId, setSelectedRunId] = useState<string | null>(restoredSelectedRunId);
  const pendingSnapshotRef = useRef<WorkspaceSnapshot | null | undefined>(undefined);
  const pendingWorkspaceRegistryRef = useRef<WorkspaceRegistryState | null>(null);
  const appliedSnapshotRef = useRef<WorkspaceSnapshot | null>(null);
  const pendingRunIdRef = useRef<{ runId: string; workspaceId: string } | null>(null);
  const snapshotActivityRef = useRef(0);
  const snapshotFrameRef = useRef<number | null>(null);
  const workspaceRegistryFrameRef = useRef<number | null>(null);

  useEffect(() => {
    try {
      if (selectedRunId) window.sessionStorage.setItem(SELECTED_RUN_STORAGE_KEY, selectedRunId);
      else window.sessionStorage.removeItem(SELECTED_RUN_STORAGE_KEY);
    } catch { /* Session selection remains usable when browser storage is unavailable. */ }
  }, [selectedRunId]);

  const applySnapshot = useCallback((next: WorkspaceSnapshot | null, selectedRunIdOverride?: string) => {
    snapshotActivityRef.current += 1;
    // A completed background summary can arrive before the original workspace-open IPC response.
    if (next && isStaleWorkspaceSnapshot(appliedSnapshotRef.current, next)) {
      if (selectedRunIdOverride !== undefined) setSelectedRunId(selectedRunIdOverride);
      return;
    }
    const previous = appliedSnapshotRef.current;
    appliedSnapshotRef.current = next;
    devInstrumentation.recordPayload('ipc.snapshot.apply', next, snapshotMetricDetail(next));
    const commit = (): void => {
      setSnapshot(next);
      if (next) {
        setOpenAiStatus(next.openAi);
      }
      setSelectedRunId((current) => selectedRunIdOverride ?? selectRunId(current, next, pendingRunIdRef.current));
    };
    if (previous?.workspace.workspacePath === next?.workspace.workspacePath
      && previous?.appServerMemory.loading === true && next?.appServerMemory.loading !== true) {
      commit();
    } else {
      startTransition(commit);
    }
  }, []);

  const selectPendingRunId = useCallback((runId: string, workspaceId: string): void => {
    pendingRunIdRef.current = { runId, workspaceId };
    setSelectedRunId(runId);
  }, []);

  const releasePendingRunId = useCallback((runId: string): void => {
    if (pendingRunIdRef.current?.runId === runId) pendingRunIdRef.current = null;
  }, []);

  useEffect(() => {
    const workspacePath = snapshot?.workspace.workspacePath;
    if (!workspacePath || snapshot?.appServerMemory.loading !== true) return undefined;
    let cancelled = false;
    let reportedError = false;
    let timer: number;
    const refresh = (): void => {
      void window.beale.getSnapshot()
        .then((next) => {
          if (cancelled || next?.workspace.workspacePath !== workspacePath
            || appliedSnapshotRef.current?.workspace.workspacePath !== workspacePath) return;
          applySnapshot(next);
        })
        .catch((caught: unknown) => {
          if (cancelled || reportedError) return;
          reportedError = true;
          onError(errorMessage(caught));
        })
        .finally(() => {
          if (!cancelled) timer = window.setTimeout(refresh, WORKSPACE_MEMORY_REFRESH_INTERVAL_MS);
        });
    };
    timer = window.setTimeout(refresh, WORKSPACE_MEMORY_REFRESH_INTERVAL_MS);
    return () => {
      cancelled = true;
      window.clearTimeout(timer);
    };
  }, [applySnapshot, onError, snapshot?.appServerMemory.loading, snapshot?.workspace.workspacePath]);

  const loadSnapshot = useCallback(async () => {
    const next = await devInstrumentation.timeAsync('ipc.getSnapshot', () => window.beale.getSnapshot());
    applySnapshot(next);
  }, [applySnapshot]);

  const loadWorkspaceRegistry = useCallback(async () => {
    const next = await devInstrumentation.timeAsync('ipc.getWorkspaceRegistry', () => window.beale.getWorkspaceRegistry());
    devInstrumentation.recordPayload('ipc.workspaceRegistry.apply', next, workspaceRegistryMetricDetail(next));
    setWorkspaceRegistry(next);
  }, []);

  useEffect(() => {
    let refreshInFlight = false;
    const refresh = (): void => {
      if (document.visibilityState !== 'visible' || refreshInFlight) return;
      refreshInFlight = true;
      void loadWorkspaceRegistry()
        .catch((caught: unknown) => onError(errorMessage(caught)))
        .finally(() => {
          refreshInFlight = false;
        });
    };
    const interval = window.setInterval(refresh, WORKSPACE_REGISTRY_REFRESH_INTERVAL_MS);
    document.addEventListener('visibilitychange', refresh);
    return () => {
      window.clearInterval(interval);
      document.removeEventListener('visibilitychange', refresh);
    };
  }, [loadWorkspaceRegistry, onError]);

  useEffect(() => {
    const unsubscribeSnapshot = window.beale.onSnapshot((next) => {
      snapshotActivityRef.current += 1;
      const applyStartedAt = performance.now();
      const detail = snapshotMetricDetail(next);
      devInstrumentation.recordPayload('ipc.snapshot.event', next, detail);
      pendingSnapshotRef.current = next;
      if (snapshotFrameRef.current === null) {
        snapshotFrameRef.current = window.requestAnimationFrame(() => {
          snapshotFrameRef.current = null;
          const latest = pendingSnapshotRef.current;
          pendingSnapshotRef.current = undefined;
          if (latest === undefined) return;
          applySnapshot(latest);
          recordNextFrameTiming('ipc.snapshot.event.apply.nextFrameLatency', applyStartedAt, snapshotMetricDetail(latest));
        });
      }
    });
    const unsubscribeWorkspaceRegistry = window.beale.onWorkspaceRegistry((next) => {
      const applyStartedAt = performance.now();
      const detail = workspaceRegistryMetricDetail(next);
      devInstrumentation.recordPayload('ipc.workspaceRegistry.event', next, detail);
      pendingWorkspaceRegistryRef.current = next;
      if (workspaceRegistryFrameRef.current === null) {
        workspaceRegistryFrameRef.current = window.requestAnimationFrame(() => {
          workspaceRegistryFrameRef.current = null;
          const latest = pendingWorkspaceRegistryRef.current;
          pendingWorkspaceRegistryRef.current = null;
          if (!latest) return;
          startTransition(() => setWorkspaceRegistry(latest));
          recordNextFrameTiming('ipc.workspaceRegistry.event.apply.nextFrameLatency', applyStartedAt, workspaceRegistryMetricDetail(latest));
        });
      }
    });
    const unsubscribeWindowChromeState = window.beale.onWindowChromeState(setWindowChromeState);
    let cancelled = false;
    const startupFrame = window.requestAnimationFrame(() => {
      if (cancelled) return;
      setStartupPhase('registry');
      const startupSnapshotActivity = snapshotActivityRef.current;
      void devInstrumentation.timeAsync('ipc.getSnapshot.initial', () => window.beale.getSnapshot())
        .then((next) => {
          if (!cancelled && next && snapshotActivityRef.current === startupSnapshotActivity) {
            applySnapshot(next);
          }
        })
        .catch((caught: unknown) => {
          if (!cancelled) onError(errorMessage(caught));
        });
      void window.beale
        .getHostEnvironment()
        .then((next) => {
          if (!cancelled) setHostEnvironment(next);
        })
        .catch((caught: unknown) => {
          if (!cancelled) onError(errorMessage(caught));
        });
      void window.beale
        .getWindowChromeState()
        .then((next) => {
          if (!cancelled) setWindowChromeState(next);
        })
        .catch((caught: unknown) => {
          if (!cancelled) onError(errorMessage(caught));
        });
      void (async () => {
        try {
          const registry = await devInstrumentation.timeAsync(
            'ipc.getWorkspaceRegistry.initial',
            () => window.beale.getWorkspaceRegistry()
          );
          if (cancelled) return;
          devInstrumentation.recordPayload(
            'ipc.workspaceRegistry.initial',
            registry,
            workspaceRegistryMetricDetail(registry)
          );
          setWorkspaceRegistry(registry);
        } catch (caught) {
          if (!cancelled) onError(errorMessage(caught));
        } finally {
          if (!cancelled) setStartupPhase('ready');
        }
      })();
    });
    return () => {
      cancelled = true;
      window.cancelAnimationFrame(startupFrame);
      unsubscribeSnapshot();
      unsubscribeWorkspaceRegistry();
      unsubscribeWindowChromeState();
      if (snapshotFrameRef.current !== null) window.cancelAnimationFrame(snapshotFrameRef.current);
      if (workspaceRegistryFrameRef.current !== null) window.cancelAnimationFrame(workspaceRegistryFrameRef.current);
      snapshotFrameRef.current = null;
      workspaceRegistryFrameRef.current = null;
      pendingSnapshotRef.current = undefined;
      pendingWorkspaceRegistryRef.current = null;
    };
  }, [applySnapshot, onError]);

  return {
    snapshot,
    workspaceRegistry,
    hostEnvironment,
    windowChromeState,
    openAiStatus,
    startupPhase,
    selectedRunId,
    setWorkspaceRegistry,
    setOpenAiStatus,
    setSelectedRunId,
    selectPendingRunId,
    releasePendingRunId,
    applySnapshot,
    loadSnapshot,
    loadWorkspaceRegistry
  };
}

function workspaceRegistryMetricDetail(registry: WorkspaceRegistryState | null): Record<string, number> {
  return {
    workspaces: registry?.workspaces.length ?? 0,
    sessions: registry?.researchSessions.length ?? 0
  };
}
