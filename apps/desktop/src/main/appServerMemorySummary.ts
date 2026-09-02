import { getAppServerMemorySummary as queryAppServerMemorySummary } from './appServerCliClient';
import type { AppServerMemorySummary, ResearchProfileSnapshot } from '@shared/types';

export interface AppServerMemorySummaryOptions {
  databasePath: string;
  artifactDirectoryPath: string;
  sessionId?: string;
  workspaceId: string;
  subjectId: string | null;
  researchProfile?: ResearchProfileSnapshot | null;
  includeForeignCatalogs?: boolean;
  assetIds?: string[];
}

/** @deprecated Use the typed app-server client directly. */
export function getAppServerMemorySummary(options: AppServerMemorySummaryOptions): AppServerMemorySummary {
  return queryAppServerMemorySummary({
    ...(options.sessionId ? { sessionId: options.sessionId } : {}),
    workspaceId: options.workspaceId,
    subjectId: options.subjectId,
    ...(options.researchProfile !== undefined ? { researchProfile: options.researchProfile } : {}),
    ...(options.includeForeignCatalogs === true ? { includeForeignCatalogs: true } : {}),
    ...(options.assetIds ? { assetIds: options.assetIds } : {})
  }, {
    databasePath: options.databasePath,
    artifactDirectoryPath: options.artifactDirectoryPath
  });
}
