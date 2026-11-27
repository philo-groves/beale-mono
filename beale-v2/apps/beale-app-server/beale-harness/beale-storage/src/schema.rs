//! Owner-scoped schema versions for the shared host database.
//!
//! The host owns the session, workspace, extension, and automation tables.
//! Each native feature owns its own tables and applies its migrations through
//! [`HostDatabase::migrate_schema`] under its feature ID, so a build without a
//! feature never creates that feature's tables. Versions are recorded per owner
//! in `schema_versions`.

use super::HostDatabase;
use anyhow::{Context, Result, bail, ensure};
use rusqlite::{Connection, OptionalExtension, params};

/// Schema owner for host tables.
pub const HOST_OWNER: &str = "host";

/// Ordered host migrations; version N is `HOST_MIGRATIONS[N - 1]`.
pub const HOST_MIGRATIONS: &[&str] = &[HOST_V1, HOST_V2];

const HOST_V2: &str = "ALTER TABLE session_turns ADD COLUMN model_prompt TEXT;";

#[test]
fn model_prompt_migration_preserves_existing_transcript_and_is_idempotent() {
    let mut db = Connection::open_in_memory().unwrap();
    ensure_registry(&db).unwrap();
    apply(&mut db, HOST_OWNER, &[HOST_V1]).unwrap();
    db.execute_batch("INSERT INTO sessions(id,workspace_id,title,created_at_ms) VALUES('session-example','workspace-example','Example',1);
        INSERT INTO session_turns(session_id,number,provider,method,model,prompt,answer) VALUES('session-example',1,1,1,'example','Original user text','Synthetic answer');").unwrap();
    for _ in 0..2 {
        apply(&mut db, HOST_OWNER, HOST_MIGRATIONS).unwrap();
        let saved: (String, Option<String>) = db
            .query_row("SELECT prompt,model_prompt FROM session_turns", [], |row| {
                Ok((row.get(0)?, row.get(1)?))
            })
            .unwrap();
        assert_eq!(saved, ("Original user text".into(), None));
    }
}

const HOST_V1: &str = "
CREATE TABLE workspaces (
    id TEXT PRIMARY KEY,
    name TEXT NOT NULL,
    directory TEXT NOT NULL UNIQUE,
    ordinal INTEGER NOT NULL UNIQUE
);
CREATE TABLE sessions (
    id TEXT PRIMARY KEY,
    workspace_id TEXT NOT NULL,
    title TEXT NOT NULL,
    created_at_ms INTEGER NOT NULL,
    archived_at_ms INTEGER,
    last_message_at_ms INTEGER NOT NULL DEFAULT 0
);
CREATE TABLE session_turns (
    session_id TEXT NOT NULL REFERENCES sessions(id) ON DELETE CASCADE,
    number INTEGER NOT NULL,
    provider INTEGER NOT NULL,
    method INTEGER NOT NULL,
    model TEXT NOT NULL,
    prompt TEXT NOT NULL,
    answer TEXT NOT NULL,
    duration_ms INTEGER NOT NULL DEFAULT 0,
    internal_prompt INTEGER NOT NULL DEFAULT 0,
    -- Top-level agent role for turns of a peer agent session; NULL otherwise.
    agent_role TEXT,
    PRIMARY KEY(session_id, number)
);
CREATE INDEX session_turns_agent_role ON session_turns(session_id, agent_role, number);
CREATE TABLE session_tool_calls (
    id TEXT PRIMARY KEY,
    session_id TEXT NOT NULL REFERENCES sessions(id) ON DELETE CASCADE,
    turn_number INTEGER NOT NULL,
    ordinal INTEGER NOT NULL,
    name TEXT NOT NULL,
    summary TEXT NOT NULL,
    arguments TEXT NOT NULL,
    output TEXT NOT NULL,
    is_error INTEGER NOT NULL,
    agent_role TEXT NOT NULL DEFAULT 'main',
    created_at_ms INTEGER NOT NULL DEFAULT 0,
    FOREIGN KEY(session_id, turn_number) REFERENCES session_turns(session_id, number),
    UNIQUE(session_id, turn_number, ordinal)
);
CREATE INDEX session_tool_calls_order ON session_tool_calls(session_id, turn_number, ordinal);
CREATE TABLE session_reasoning_summaries (
    session_id TEXT NOT NULL,
    turn_number INTEGER NOT NULL,
    ordinal INTEGER NOT NULL,
    after_tool_count INTEGER NOT NULL,
    text TEXT NOT NULL,
    agent_role TEXT NOT NULL DEFAULT 'main',
    PRIMARY KEY(session_id, turn_number, ordinal),
    FOREIGN KEY(session_id, turn_number) REFERENCES session_turns(session_id, number) ON DELETE CASCADE
);
CREATE TABLE session_commentary (
    session_id TEXT NOT NULL,
    turn_number INTEGER NOT NULL,
    ordinal INTEGER NOT NULL,
    after_tool_count INTEGER NOT NULL,
    text TEXT NOT NULL,
    agent_role TEXT NOT NULL DEFAULT 'main',
    PRIMARY KEY(session_id, turn_number, ordinal),
    FOREIGN KEY(session_id, turn_number) REFERENCES session_turns(session_id, number) ON DELETE CASCADE
);
CREATE TABLE session_child_turns (
    session_id TEXT NOT NULL,
    parent_turn INTEGER NOT NULL,
    agent_id TEXT NOT NULL,
    role TEXT NOT NULL,
    task TEXT NOT NULL,
    result TEXT NOT NULL,
    is_error INTEGER NOT NULL,
    messages_json TEXT NOT NULL,
    reasoning_json TEXT NOT NULL,
    completed_at_ms INTEGER NOT NULL,
    PRIMARY KEY(session_id, parent_turn, agent_id),
    FOREIGN KEY(session_id, parent_turn) REFERENCES session_turns(session_id, number) ON DELETE CASCADE
);
CREATE TABLE session_child_tools (
    tool_call_id TEXT PRIMARY KEY REFERENCES session_tool_calls(id) ON DELETE CASCADE,
    agent_id TEXT NOT NULL
);
CREATE TABLE session_model_usage (
    session_id TEXT NOT NULL,
    turn_number INTEGER NOT NULL,
    ordinal INTEGER NOT NULL,
    agent_id TEXT NOT NULL,
    role TEXT NOT NULL,
    provider INTEGER NOT NULL,
    method INTEGER NOT NULL,
    model TEXT NOT NULL,
    input_tokens INTEGER,
    output_tokens INTEGER,
    duration_ms INTEGER NOT NULL,
    observed_at_ms INTEGER NOT NULL,
    cache_read_tokens INTEGER,
    work_elapsed_ms INTEGER,
    PRIMARY KEY(session_id, turn_number, ordinal),
    FOREIGN KEY(session_id, turn_number) REFERENCES session_turns(session_id, number) ON DELETE CASCADE
);
CREATE INDEX session_model_usage_observed ON session_model_usage(observed_at_ms);
-- Replayable provider context per agent: '' for the session's main agent,
-- or a top-level role in a peer agent session.
CREATE TABLE session_provider_context (
    session_id TEXT NOT NULL REFERENCES sessions(id) ON DELETE CASCADE,
    agent_role TEXT NOT NULL DEFAULT '',
    context_json TEXT NOT NULL,
    PRIMARY KEY(session_id, agent_role)
);
CREATE TABLE quick_chat_contexts (
    session_id TEXT PRIMARY KEY REFERENCES sessions(id) ON DELETE CASCADE,
    workspace_id TEXT NOT NULL
);
CREATE TABLE provider_settings (
    key TEXT PRIMARY KEY,
    value TEXT NOT NULL
);
CREATE TABLE permission_rules (
    id TEXT PRIMARY KEY,
    instruction TEXT NOT NULL,
    enabled INTEGER NOT NULL,
    title TEXT NOT NULL DEFAULT '',
    stage INTEGER,
    position INTEGER
);
CREATE TABLE plugin_installs (
    name TEXT PRIMARY KEY,
    installed_at_ms INTEGER NOT NULL
);
CREATE TABLE feature_states (
    id TEXT PRIMARY KEY,
    enabled INTEGER NOT NULL CHECK (enabled IN (0, 1))
);
CREATE TABLE extension_scope_modes (
    kind TEXT NOT NULL CHECK (kind IN ('plugin', 'feature')),
    extension_id TEXT NOT NULL,
    all_workspaces INTEGER NOT NULL CHECK (all_workspaces IN (0, 1)),
    PRIMARY KEY(kind, extension_id)
);
CREATE TABLE extension_workspace_scope (
    kind TEXT NOT NULL CHECK (kind IN ('plugin', 'feature')),
    extension_id TEXT NOT NULL,
    workspace_id TEXT NOT NULL,
    PRIMARY KEY(kind, extension_id, workspace_id)
);
CREATE INDEX extension_workspace_scope_workspace ON extension_workspace_scope(workspace_id);
CREATE TABLE automation_jobs (
    id TEXT PRIMARY KEY,
    session_id TEXT NOT NULL UNIQUE REFERENCES sessions(id) ON DELETE CASCADE,
    workspace_id TEXT NOT NULL,
    title TEXT NOT NULL,
    repeat_preset TEXT NOT NULL,
    next_run_at_ms INTEGER NOT NULL,
    last_run_at_ms INTEGER NOT NULL,
    next_occurrence INTEGER NOT NULL DEFAULT 1,
    canceled INTEGER NOT NULL DEFAULT 0 CHECK (canceled IN (0, 1))
);
CREATE INDEX automation_jobs_next_run ON automation_jobs(canceled, next_run_at_ms);
CREATE TABLE automation_job_options (
    job_id TEXT PRIMARY KEY REFERENCES automation_jobs(id) ON DELETE CASCADE,
    reasoning_effort INTEGER NOT NULL,
    goal_mode INTEGER NOT NULL CHECK (goal_mode IN (0, 1)),
    goal_max_turns INTEGER NOT NULL,
    goal_max_duration_minutes INTEGER NOT NULL,
    anchor_at_ms INTEGER NOT NULL
);
CREATE TABLE automation_runs (
    id TEXT PRIMARY KEY,
    job_id TEXT NOT NULL REFERENCES automation_jobs(id) ON DELETE CASCADE,
    session_id TEXT REFERENCES sessions(id) ON DELETE SET NULL,
    title TEXT NOT NULL,
    started_at_ms INTEGER NOT NULL,
    completed_at_ms INTEGER,
    status INTEGER NOT NULL CHECK (status IN (1, 2, 3, 4)),
    lease_expires_at_ms INTEGER NOT NULL DEFAULT 0,
    error TEXT NOT NULL DEFAULT ''
);
CREATE INDEX automation_runs_job_status ON automation_runs(job_id, status);
";

/// Rejects databases created before owner-scoped schemas, which used a single
/// `user_version` chain. Those development databases are not migrated.
pub(super) fn reject_legacy(connection: &Connection) -> Result<()> {
    let version: i64 = connection.query_row("PRAGMA user_version", [], |row| row.get(0))?;
    if version != 0 {
        bail!(
            "state.sqlite was created by an earlier Beale v2 development build (schema {version}) \
             and cannot be upgraded; move it aside or delete it, then restart the app-server"
        );
    }
    Ok(())
}

pub(super) fn ensure_registry(connection: &Connection) -> Result<()> {
    connection.execute_batch(
        "CREATE TABLE IF NOT EXISTS schema_versions (
             owner TEXT PRIMARY KEY,
             version INTEGER NOT NULL CHECK (version >= 0)
         );",
    )?;
    Ok(())
}

pub(super) fn apply(connection: &mut Connection, owner: &str, migrations: &[&str]) -> Result<()> {
    ensure!(
        !owner.is_empty() && owner.len() <= 64,
        "invalid schema owner"
    );
    let transaction = connection.transaction()?;
    let current: i64 = transaction
        .query_row(
            "SELECT version FROM schema_versions WHERE owner = ?1",
            [owner],
            |row| row.get(0),
        )
        .optional()?
        .unwrap_or(0);
    let target = i64::try_from(migrations.len()).context("too many migrations")?;
    if current > target {
        bail!("unsupported {owner} schema version {current}; this build supports {target}");
    }
    for (index, sql) in migrations.iter().enumerate().skip(current as usize) {
        transaction
            .execute_batch(sql)
            .with_context(|| format!("apply {owner} schema version {}", index + 1))?;
    }
    if current < target {
        transaction.execute(
            "INSERT INTO schema_versions(owner, version) VALUES (?1, ?2)
             ON CONFLICT(owner) DO UPDATE SET version = excluded.version",
            params![owner, target],
        )?;
    }
    transaction.commit()?;
    Ok(())
}

impl HostDatabase {
    /// Applies an owner's ordered migrations that have not run yet. Version N is
    /// `migrations[N - 1]`; a database newer than the supplied list is rejected.
    pub fn migrate_schema(&self, owner: &str, migrations: &[&str]) -> Result<()> {
        ensure!(owner != HOST_OWNER, "the host schema is applied on open");
        let mut connection = self
            .0
            .lock()
            .map_err(|_| anyhow::anyhow!("database lock poisoned"))?;
        if self.1 {
            let version: Option<i64> = connection
                .query_row(
                    "SELECT version FROM schema_versions WHERE owner = ?1",
                    [owner],
                    |r| r.get(0),
                )
                .optional()?;
            ensure!(
                version == Some(i64::try_from(migrations.len())?),
                "schema unavailable in read snapshot"
            );
            return Ok(());
        }
        apply(&mut connection, owner, migrations)
    }

    /// The recorded schema version for an owner, or zero when none ran.
    pub fn schema_version(&self, owner: &str) -> Result<i64> {
        self.with_connection(|connection| {
            Ok(connection
                .query_row(
                    "SELECT version FROM schema_versions WHERE owner = ?1",
                    [owner],
                    |row| row.get(0),
                )
                .optional()?
                .unwrap_or(0))
        })
    }

    /// Runs statements that restore feature state after a host restart, such as
    /// marking work that was running when the host stopped as interrupted.
    pub fn run_startup_statements(&self, owner: &str, statements: &[&str]) -> Result<()> {
        self.with_connection(|connection| {
            for statement in statements {
                connection
                    .execute_batch(statement)
                    .with_context(|| format!("run {owner} startup statement"))?;
            }
            Ok(())
        })
    }
}
