import assert from "node:assert/strict";
import { mkdtemp, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { DatabaseSync } from "node:sqlite";
import test from "node:test";

import { applyDatabaseMigrations } from "../packages/research-agent/dist/index.js";

test("component migrations are append-only and transactional", async () => {
  const directory = await mkdtemp(join(tmpdir(), "app-server-migrations-"));
  const database = new DatabaseSync(join(directory, "memory.sqlite"));
  try {
    applyDatabaseMigrations(database, "fixture", [{
      version: 1,
      name: "baseline",
      up(target) {
        target.exec("CREATE TABLE durable_rows (id TEXT PRIMARY KEY); INSERT INTO durable_rows VALUES ('preserved');");
      },
    }]);

    assert.throws(() => applyDatabaseMigrations(database, "fixture", [
      { version: 1, name: "baseline", up() {} },
      {
        version: 2,
        name: "broken_change",
        up(target) {
          target.exec("INSERT INTO durable_rows VALUES ('rolled_back');");
          throw new Error("fixture failure");
        },
      },
    ]), /broken_change/);
    assert.deepEqual(database.prepare("SELECT id FROM durable_rows ORDER BY id").all().map((row) => ({ ...row })), [{ id: "preserved" }]);
    assert.deepEqual(database.prepare("SELECT version, name FROM schema_migrations WHERE component = 'fixture'").all().map((row) => ({ ...row })), [{ version: 1, name: "baseline" }]);
    assert.throws(() => applyDatabaseMigrations(database, "fixture", [{ version: 1, name: "renamed", up() {} }]), /renamed/);
    assert.throws(() => applyDatabaseMigrations(database, "gap", [{ version: 2, name: "missing_baseline", up() {} }]), /contiguous/);
  } finally {
    database.close();
    await rm(directory, { recursive: true, force: true });
  }
});

test("component migrations adopt pre-Beale table and history names in place", async () => {
  const directory = await mkdtemp(join(tmpdir(), "app-server-legacy-migrations-"));
  const database = new DatabaseSync(join(directory, "memory.sqlite"));
  const previousPrefix = `${["honey", "crisp"].join("")}_`;
  try {
    database.exec(`
      CREATE TABLE schema_migrations (
        component TEXT NOT NULL,
        version INTEGER NOT NULL,
        name TEXT NOT NULL,
        applied_at TEXT NOT NULL,
        PRIMARY KEY(component, version)
      );
      CREATE TABLE ${previousPrefix}sessions (id TEXT PRIMARY KEY, title TEXT NOT NULL);
      CREATE INDEX ${previousPrefix}sessions_title ON ${previousPrefix}sessions(title);
      INSERT INTO ${previousPrefix}sessions VALUES ('session_preserved', 'Preserved');
    `);
    database.prepare("INSERT INTO schema_migrations VALUES (?, 1, ?, ?)")
      .run(`${previousPrefix}sessions`, `${previousPrefix}baseline`, new Date().toISOString());

    applyDatabaseMigrations(database, "app_server_sessions", [{
      version: 1,
      name: "app_server_baseline",
      up() {
        throw new Error("An adopted migration must not run again.");
      },
    }]);

    assert.deepEqual(
      database.prepare("SELECT id, title FROM app_server_sessions").all().map((row) => ({ ...row })),
      [{ id: "session_preserved", title: "Preserved" }],
    );
    assert.deepEqual(
      database.prepare("SELECT component, name FROM schema_migrations").all().map((row) => ({ ...row })),
      [{ component: "app_server_sessions", name: "app_server_baseline" }],
    );
    assert.equal(
      database.prepare("SELECT name FROM sqlite_master WHERE type = 'index' AND name = 'app_server_sessions_title'").get()?.name,
      "app_server_sessions_title",
    );
  } finally {
    database.close();
    await rm(directory, { recursive: true, force: true });
  }
});
