import type { DatabaseSync } from "node:sqlite";
import { preBealeRuntimeId } from "./legacy-compatibility.js";

export interface DatabaseMigration {
  version: number;
  name: string;
  up(database: DatabaseSync): void;
}

export function applyDatabaseMigrations(
  database: DatabaseSync,
  component: string,
  migrations: readonly DatabaseMigration[],
): void {
  const ordered = [...migrations].sort((left, right) => left.version - right.version);
  validateMigrationDefinitions(component, ordered);
  database.exec(`
    CREATE TABLE IF NOT EXISTS schema_migrations (
      component TEXT NOT NULL,
      version INTEGER NOT NULL CHECK (version > 0),
      name TEXT NOT NULL,
      applied_at TEXT NOT NULL,
      PRIMARY KEY(component, version)
    );
  `);
  migratePreBealeDatabaseNames(database);

  const appliedRows = database
    .prepare("SELECT version, name FROM schema_migrations WHERE component = ? ORDER BY version")
    .all(component) as Array<{ version: number; name: string }>;
  if (appliedRows.some((row, index) => row.version !== index + 1)) {
    throw new Error(`Database migration history for ${component} is not contiguous.`);
  }
  const applied = new Map(appliedRows.map((row) => [row.version, row.name]));
  const latestVersion = ordered.at(-1)?.version ?? 0;
  const unknownVersion = [...applied.keys()].find((version) => version > latestVersion || !ordered.some((migration) => migration.version === version));
  if (unknownVersion !== undefined) {
    throw new Error(`Database schema for ${component} is newer or unknown at migration ${unknownVersion}.`);
  }

  for (const migration of ordered) {
    const appliedName = applied.get(migration.version);
    if (appliedName !== undefined) {
      if (appliedName !== migration.name) {
        throw new Error(`Migration ${component}:${migration.version} was renamed from ${appliedName} to ${migration.name}.`);
      }
      continue;
    }

    database.exec("BEGIN IMMEDIATE");
    try {
      migration.up(database);
      database
        .prepare("INSERT INTO schema_migrations(component, version, name, applied_at) VALUES (?, ?, ?, ?)")
        .run(component, migration.version, migration.name, new Date().toISOString());
      database.exec("COMMIT");
    } catch (error) {
      database.exec("ROLLBACK");
      const message = error instanceof Error ? error.message : String(error);
      throw new Error(`Migration ${component}:${migration.version} (${migration.name}) failed: ${message}`, { cause: error });
    }
  }
}

function migratePreBealeDatabaseNames(database: DatabaseSync): void {
  const previousPrefix = `${preBealeRuntimeId()}_`;
  const currentPrefix = "app_server_";
  const objects = database.prepare(`
    SELECT type, name, sql
    FROM sqlite_master
    WHERE name LIKE ? AND type IN ('table', 'index', 'trigger', 'view')
    ORDER BY CASE type WHEN 'view' THEN 0 WHEN 'trigger' THEN 1 WHEN 'index' THEN 2 ELSE 3 END
  `).all(`${previousPrefix}%`) as Array<{ type: string; name: string; sql: string | null }>;
  const migrationRows = database.prepare(`
    SELECT 1
    FROM schema_migrations
    WHERE instr(component, ?) > 0 OR instr(name, ?) > 0
    LIMIT 1
  `).get(preBealeRuntimeId(), preBealeRuntimeId());
  if (objects.length === 0 && !migrationRows) return;

  database.exec("BEGIN IMMEDIATE");
  try {
    const recreated = objects.filter((object) => object.type !== "table" && object.sql);
    for (const object of recreated) {
      database.exec(`DROP ${object.type.toUpperCase()} IF EXISTS ${quotedIdentifier(object.name)}`);
    }
    for (const object of objects.filter((candidate) => candidate.type === "table")) {
      const nextName = object.name.replace(previousPrefix, currentPrefix);
      const collision = database.prepare("SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = ?").get(nextName);
      if (collision) {
        throw new Error(`Cannot adopt the pre-Beale table ${object.name}: ${nextName} already exists.`);
      }
      database.exec(`ALTER TABLE ${quotedIdentifier(object.name)} RENAME TO ${quotedIdentifier(nextName)}`);
    }
    for (const object of recreated.reverse()) {
      database.exec(object.sql!.split(preBealeRuntimeId()).join("app_server"));
    }
    database.prepare(`
      UPDATE schema_migrations
      SET component = replace(component, ?, ?), name = replace(name, ?, ?)
      WHERE instr(component, ?) > 0 OR instr(name, ?) > 0
    `).run(
      preBealeRuntimeId(), "app_server",
      preBealeRuntimeId(), "app_server",
      preBealeRuntimeId(), preBealeRuntimeId(),
    );
    database.exec("COMMIT");
  } catch (error) {
    database.exec("ROLLBACK");
    throw error;
  }
}

function quotedIdentifier(value: string): string {
  return `"${value.replaceAll('"', '""')}"`;
}

function validateMigrationDefinitions(component: string, migrations: readonly DatabaseMigration[]): void {
  const versions = new Set<number>();
  for (const [index, migration] of migrations.entries()) {
    if (!Number.isInteger(migration.version) || migration.version <= 0) {
      throw new Error(`Migration versions for ${component} must be positive integers.`);
    }
    if (versions.has(migration.version)) {
      throw new Error(`Duplicate migration version for ${component}: ${migration.version}.`);
    }
    if (migration.version !== index + 1) {
      throw new Error(`Migration versions for ${component} must be contiguous starting at 1.`);
    }
    if (!migration.name.trim()) {
      throw new Error(`Migration ${component}:${migration.version} requires a name.`);
    }
    versions.add(migration.version);
  }
}
