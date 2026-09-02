import { createRequire } from "node:module";
import type { DatabaseSync } from "node:sqlite";

const require = createRequire(import.meta.url);

export interface ResearchDatabaseOpenOptions {
  readOnly?: boolean;
}

export type ResearchDatabaseFactory = (
  path: string,
  options?: ResearchDatabaseOpenOptions,
) => DatabaseSync;

let installedFactory: ResearchDatabaseFactory | undefined;

/**
 * Installs the database boundary used by research stores in the current JS
 * isolate. The app-server runtime worker uses this to delegate every SQLite
 * operation to its host; normal library consumers retain the local default.
 */
export function installResearchDatabaseFactory(
  factory: ResearchDatabaseFactory | undefined,
): () => void {
  const previous = installedFactory;
  installedFactory = factory;
  return () => {
    installedFactory = previous;
  };
}

export function openResearchDatabase(
  path: string,
  options?: ResearchDatabaseOpenOptions,
): DatabaseSync {
  if (installedFactory) return installedFactory(path, options);
  const { DatabaseSync } = require("node:sqlite") as {
    DatabaseSync: new (
      path: string,
      options?: ResearchDatabaseOpenOptions,
    ) => DatabaseSync;
  };
  return options ? new DatabaseSync(path, options) : new DatabaseSync(path);
}
