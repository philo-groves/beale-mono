use anyhow::{Context, Result, bail, ensure};
use rusqlite::{Connection, OptionalExtension, params};
use std::{
    path::Path,
    sync::{Arc, Mutex},
    time::Duration,
};

#[derive(Clone)]
pub struct HostDatabase(Arc<Mutex<Connection>>);

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WorkspaceRow {
    pub id: String,
    pub name: String,
    pub directory: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CustomPermissionRuleRow {
    pub id: String,
    pub stage: i32,
    pub title: String,
    pub instruction: String,
    pub enabled: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SessionRow {
    pub id: String,
    pub workspace_id: String,
    pub title: String,
    pub created_at_ms: i64,
    pub last_message_at_ms: i64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ArchiveRow {
    pub id: String,
    pub workspace_id: String,
    pub title: String,
    pub model: String,
    pub turn_count: i64,
    pub created_at_ms: i64,
    pub archived_at_ms: i64,
    pub quick_chat: bool,
    pub last_message_at_ms: i64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TurnRow {
    pub number: i64,
    pub provider: i32,
    pub method: i32,
    pub model: String,
    pub prompt: String,
    pub answer: String,
    pub duration_ms: i64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ToolCallRow {
    pub id: String,
    pub turn_number: i64,
    pub ordinal: i64,
    pub name: String,
    pub summary: String,
    pub arguments: String,
    pub output: String,
    pub is_error: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ToolCallSummaryRow {
    pub id: String,
    pub turn_number: i64,
    pub name: String,
    pub summary: String,
    pub is_error: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReasoningSummaryRow {
    pub turn_number: i64,
    pub ordinal: i64,
    pub after_tool_count: i64,
    pub text: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CommentaryRow {
    pub turn_number: i64,
    pub ordinal: i64,
    pub after_tool_count: i64,
    pub text: String,
}

#[derive(Clone, Debug)]
pub struct SessionWithTurns {
    pub session: SessionRow,
    pub turns: Vec<TurnRow>,
}

impl HostDatabase {
    pub fn open(root: &Path) -> Result<Self> {
        private_root(root)?;
        let path = root.join("state.sqlite");
        let connection = Connection::open(&path).context("open host database")?;
        #[cfg(windows)]
        restrict_windows_acl(&path, false)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600))
                .context("restrict host database")?;
        }
        connection.busy_timeout(Duration::from_secs(5))?;
        connection.execute_batch("PRAGMA foreign_keys = ON;")?;
        let version: i64 = connection.query_row("PRAGMA user_version", [], |row| row.get(0))?;
        if version > 12 {
            bail!("unsupported host database schema version {version}");
        }
        connection.execute_batch("PRAGMA journal_mode = WAL; PRAGMA synchronous = FULL;")?;
        if version == 0 {
            connection.execute_batch(
                "BEGIN IMMEDIATE;
                 CREATE TABLE IF NOT EXISTS workspaces (
                     id TEXT PRIMARY KEY, name TEXT NOT NULL, directory TEXT NOT NULL UNIQUE,
                     ordinal INTEGER NOT NULL UNIQUE
                 );
                 CREATE TABLE IF NOT EXISTS sessions (
                     id TEXT PRIMARY KEY, workspace_id TEXT NOT NULL, title TEXT NOT NULL,
                     created_at_ms INTEGER NOT NULL
                 );
                 CREATE TABLE IF NOT EXISTS session_turns (
                     session_id TEXT NOT NULL REFERENCES sessions(id) ON DELETE CASCADE,
                     number INTEGER NOT NULL, provider INTEGER NOT NULL, method INTEGER NOT NULL,
                     model TEXT NOT NULL, prompt TEXT NOT NULL, answer TEXT NOT NULL,
                     PRIMARY KEY(session_id, number)
                 );
                 CREATE TABLE IF NOT EXISTS provider_settings (
                     key TEXT PRIMARY KEY, value TEXT NOT NULL
                 );
                 PRAGMA user_version = 1;
                 COMMIT;",
            )?;
        }
        if version < 2 {
            connection.execute_batch(
                "BEGIN IMMEDIATE;
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
                     FOREIGN KEY(session_id, turn_number) REFERENCES session_turns(session_id, number),
                     UNIQUE(session_id, turn_number, ordinal)
                 );
                 CREATE INDEX session_tool_calls_order ON session_tool_calls(session_id, turn_number, ordinal);
                 PRAGMA user_version = 2;
                 COMMIT;",
            )?;
        }
        if version < 3 {
            connection.execute_batch(
                "BEGIN IMMEDIATE;
                 CREATE TABLE session_reasoning_summaries (
                     session_id TEXT NOT NULL,
                     turn_number INTEGER NOT NULL,
                     ordinal INTEGER NOT NULL,
                     after_tool_count INTEGER NOT NULL,
                     text TEXT NOT NULL,
                     PRIMARY KEY(session_id, turn_number, ordinal),
                     FOREIGN KEY(session_id, turn_number) REFERENCES session_turns(session_id, number) ON DELETE CASCADE
                 );
                 PRAGMA user_version = 3;
                 COMMIT;",
            )?;
        }
        if version < 4 {
            connection.execute_batch(
                "BEGIN IMMEDIATE;
                 ALTER TABLE session_turns ADD COLUMN duration_ms INTEGER NOT NULL DEFAULT 0;
                 PRAGMA user_version = 4;
                 COMMIT;",
            )?;
        }
        if version < 5 {
            connection.execute_batch(
                "BEGIN IMMEDIATE;
                 CREATE TABLE session_commentary (
                     session_id TEXT NOT NULL,
                     turn_number INTEGER NOT NULL,
                     ordinal INTEGER NOT NULL,
                     after_tool_count INTEGER NOT NULL,
                     text TEXT NOT NULL,
                     PRIMARY KEY(session_id, turn_number, ordinal),
                     FOREIGN KEY(session_id, turn_number) REFERENCES session_turns(session_id, number) ON DELETE CASCADE
                 );
                 PRAGMA user_version = 5;
                 COMMIT;",
            )?;
        }
        if version < 6 {
            connection.execute_batch(
                "BEGIN IMMEDIATE;
                 CREATE TABLE quick_chat_contexts (
                     session_id TEXT PRIMARY KEY REFERENCES sessions(id) ON DELETE CASCADE,
                     workspace_id TEXT NOT NULL
                 );
                 INSERT INTO quick_chat_contexts(session_id, workspace_id)
                     SELECT id, '' FROM sessions WHERE workspace_id = 'internal:quick-chats';
                 PRAGMA user_version = 6;
                 COMMIT;",
            )?;
        }
        if version < 7 {
            connection.execute_batch(
                "BEGIN IMMEDIATE;
                 ALTER TABLE sessions ADD COLUMN archived_at_ms INTEGER;
                 UPDATE sessions SET archived_at_ms = created_at_ms
                     WHERE workspace_id = 'internal:quick-chats';
                 PRAGMA user_version = 7;
                 COMMIT;",
            )?;
        }
        if version < 8 {
            connection.execute_batch(
                "BEGIN IMMEDIATE;
                 CREATE TABLE IF NOT EXISTS permission_rules (
                     id TEXT PRIMARY KEY, instruction TEXT NOT NULL, enabled INTEGER NOT NULL
                 );
                 PRAGMA user_version = 8;
                 COMMIT;",
            )?;
        }
        if version < 9 {
            connection.execute_batch(
                "BEGIN IMMEDIATE;
                 ALTER TABLE permission_rules ADD COLUMN title TEXT NOT NULL DEFAULT '';
                 PRAGMA user_version = 9;
                 COMMIT;",
            )?;
        }
        if version < 10 {
            connection.execute_batch(
                "BEGIN IMMEDIATE;
                 ALTER TABLE permission_rules ADD COLUMN stage INTEGER;
                 ALTER TABLE permission_rules ADD COLUMN position INTEGER;
                 PRAGMA user_version = 10;
                 COMMIT;",
            )?;
        }
        if version < 11 {
            connection.execute_batch(
                "BEGIN IMMEDIATE;
                 CREATE TABLE plugin_installs (
                     name TEXT PRIMARY KEY, installed_at_ms INTEGER NOT NULL
                 );
                 PRAGMA user_version = 11;
                 COMMIT;",
            )?;
        }
        if version < 12 {
            connection.execute_batch(
                "BEGIN IMMEDIATE;
                 ALTER TABLE sessions ADD COLUMN last_message_at_ms INTEGER NOT NULL DEFAULT 0;
                 UPDATE sessions SET last_message_at_ms = created_at_ms;
                 PRAGMA user_version = 12;
                 COMMIT;",
            )?;
        }
        Ok(Self(Arc::new(Mutex::new(connection))))
    }

    fn with_connection<T>(&self, action: impl FnOnce(&mut Connection) -> Result<T>) -> Result<T> {
        let mut connection = self
            .0
            .lock()
            .map_err(|_| anyhow::anyhow!("host database lock poisoned"))?;
        action(&mut connection)
    }

    pub fn installed_plugin_names(&self) -> Result<Vec<String>> {
        self.with_connection(|connection| {
            let mut statement =
                connection.prepare("SELECT name FROM plugin_installs ORDER BY name")?;
            let rows = statement.query_map([], |row| row.get(0))?;
            rows.collect::<rusqlite::Result<Vec<_>>>()
                .map_err(Into::into)
        })
    }

    pub fn install_plugin(&self, name: &str, installed_at_ms: i64) -> Result<bool> {
        self.with_connection(|connection| {
            Ok(connection.execute(
                "INSERT OR IGNORE INTO plugin_installs(name, installed_at_ms) VALUES (?1, ?2)",
                params![name, installed_at_ms],
            )? > 0)
        })
    }

    pub fn uninstall_plugin(&self, name: &str) -> Result<bool> {
        self.with_connection(|connection| {
            Ok(connection.execute("DELETE FROM plugin_installs WHERE name = ?1", [name])? > 0)
        })
    }

    pub fn workspaces(&self) -> Result<Vec<WorkspaceRow>> {
        self.with_connection(|connection| {
            let mut statement = connection
                .prepare("SELECT id, name, directory FROM workspaces ORDER BY ordinal")?;
            let rows = statement.query_map([], |row| {
                Ok(WorkspaceRow {
                    id: row.get(0)?,
                    name: row.get(1)?,
                    directory: row.get(2)?,
                })
            })?;
            rows.collect::<rusqlite::Result<Vec<_>>>()
                .map_err(Into::into)
        })
    }

    pub fn replace_workspaces(&self, rows: &[WorkspaceRow]) -> Result<()> {
        self.with_connection(|connection| {
            let transaction = connection.transaction()?;
            transaction.execute("DELETE FROM workspaces", [])?;
            for (ordinal, row) in rows.iter().enumerate() {
                transaction.execute(
                    "INSERT INTO workspaces(id, name, directory, ordinal) VALUES (?1, ?2, ?3, ?4)",
                    params![row.id, row.name, row.directory, i64::try_from(ordinal)?],
                )?;
            }
            transaction.commit()?;
            Ok(())
        })
    }

    pub fn sessions(&self) -> Result<Vec<SessionRow>> {
        self.with_connection(|connection| {
            let mut statement = connection.prepare(
                "SELECT id, workspace_id, title, created_at_ms, last_message_at_ms FROM sessions
                 ORDER BY created_at_ms DESC LIMIT 1025",
            )?;
            let rows = statement.query_map([], session_from_row)?;
            rows.collect::<rusqlite::Result<Vec<_>>>()
                .map_err(Into::into)
        })
    }

    pub fn sessions_excluding(&self, workspace_id: &str) -> Result<Vec<SessionRow>> {
        self.with_connection(|connection| {
            let mut statement = connection.prepare(
                "SELECT id, workspace_id, title, created_at_ms, last_message_at_ms FROM sessions
                 WHERE workspace_id != ?1 AND archived_at_ms IS NULL
                 ORDER BY created_at_ms DESC LIMIT 1025",
            )?;
            let rows = statement.query_map([workspace_id], session_from_row)?;
            rows.collect::<rusqlite::Result<Vec<_>>>()
                .map_err(Into::into)
        })
    }

    pub fn archive_session(&self, id: &str, archived_at_ms: i64) -> Result<bool> {
        self.with_connection(|connection| {
            Ok(connection.execute(
                "UPDATE sessions SET archived_at_ms = ?2
                 WHERE id = ?1 AND workspace_id != 'internal:quick-chats'",
                params![id, archived_at_ms],
            )? > 0)
        })
    }

    pub fn restore_session(&self, id: &str) -> Result<bool> {
        self.with_connection(|connection| {
            Ok(connection.execute(
                "UPDATE sessions SET archived_at_ms = NULL
                 WHERE id = ?1 AND workspace_id != 'internal:quick-chats'
                 AND archived_at_ms IS NOT NULL",
                [id],
            )? > 0)
        })
    }

    pub fn archive_items(&self) -> Result<Vec<ArchiveRow>> {
        self.with_connection(|connection| {
            let mut statement = connection.prepare(
                "SELECT s.id, COALESCE(q.workspace_id, s.workspace_id), s.title,
                        COALESCE((SELECT model FROM session_turns
                                  WHERE session_id = s.id ORDER BY number DESC LIMIT 1), ''),
                        (SELECT COUNT(*) FROM session_turns WHERE session_id = s.id),
                        s.created_at_ms, s.archived_at_ms,
                        s.workspace_id = 'internal:quick-chats', s.last_message_at_ms
                 FROM sessions s
                 LEFT JOIN quick_chat_contexts q ON q.session_id = s.id
                 WHERE s.archived_at_ms IS NOT NULL
                 ORDER BY s.archived_at_ms DESC, s.id DESC LIMIT 1025",
            )?;
            let rows = statement.query_map([], |row| {
                Ok(ArchiveRow {
                    id: row.get(0)?,
                    workspace_id: row.get(1)?,
                    title: row.get(2)?,
                    model: row.get(3)?,
                    turn_count: row.get(4)?,
                    created_at_ms: row.get(5)?,
                    archived_at_ms: row.get(6)?,
                    quick_chat: row.get(7)?,
                    last_message_at_ms: row.get(8)?,
                })
            })?;
            rows.collect::<rusqlite::Result<Vec<_>>>()
                .map_err(Into::into)
        })
    }

    pub fn archived_quick_chat_ids(&self) -> Result<Vec<String>> {
        self.with_connection(|connection| {
            let mut statement = connection.prepare(
                "SELECT id FROM sessions WHERE workspace_id = 'internal:quick-chats'
                 AND archived_at_ms IS NOT NULL ORDER BY created_at_ms DESC LIMIT 1025",
            )?;
            let rows = statement.query_map([], |row| row.get(0))?;
            rows.collect::<rusqlite::Result<Vec<_>>>()
                .map_err(Into::into)
        })
    }

    pub fn session(&self, id: &str) -> Result<Option<SessionWithTurns>> {
        self.with_connection(|connection| {
            let session = connection
                .query_row(
                    "SELECT id, workspace_id, title, created_at_ms, last_message_at_ms FROM sessions WHERE id = ?1",
                    [id],
                    session_from_row,
                )
                .optional()?;
            let Some(session) = session else {
                return Ok(None);
            };
            let mut statement = connection.prepare(
                "SELECT number, provider, method, model, prompt, answer, duration_ms FROM session_turns
                 WHERE session_id = ?1 ORDER BY number LIMIT 33",
            )?;
            let rows = statement.query_map([id], |row| {
                Ok(TurnRow {
                    number: row.get(0)?,
                    provider: row.get(1)?,
                    method: row.get(2)?,
                    model: row.get(3)?,
                    prompt: row.get(4)?,
                    answer: row.get(5)?,
                    duration_ms: row.get(6)?,
                })
            })?;
            Ok(Some(SessionWithTurns {
                session,
                turns: rows.collect::<rusqlite::Result<Vec<_>>>()?,
            }))
        })
    }

    pub fn append_turn(
        &self,
        session: &SessionRow,
        turn: &TurnRow,
        new_session: bool,
    ) -> Result<()> {
        self.append_turn_with_tools(session, turn, new_session, &[])
    }

    pub fn quick_chat_workspace(&self, session_id: &str) -> Result<Option<String>> {
        self.with_connection(|connection| {
            connection
                .query_row(
                    "SELECT workspace_id FROM quick_chat_contexts WHERE session_id = ?1",
                    [session_id],
                    |row| row.get(0),
                )
                .optional()
                .map_err(Into::into)
        })
    }

    pub fn append_new_quick_chat_turn(
        &self,
        session: &SessionRow,
        turn: &TurnRow,
        workspace_id: &str,
    ) -> Result<()> {
        self.with_connection(|connection| {
            let transaction = connection.transaction()?;
            transaction.execute(
                "INSERT INTO sessions(id, workspace_id, title, created_at_ms, archived_at_ms, last_message_at_ms)
                 VALUES (?1, ?2, ?3, ?4, ?4, ?5)",
                params![session.id, session.workspace_id, session.title, session.created_at_ms, session.last_message_at_ms],
            )?;
            transaction.execute(
                "INSERT INTO quick_chat_contexts(session_id, workspace_id) VALUES (?1, ?2)",
                params![session.id, workspace_id],
            )?;
            transaction.execute(
                "INSERT INTO session_turns(session_id, number, provider, method, model, prompt, answer, duration_ms)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
                params![session.id, turn.number, turn.provider, turn.method, turn.model, turn.prompt, turn.answer, turn.duration_ms],
            )?;
            transaction.commit()?;
            Ok(())
        })
    }

    pub fn append_turn_with_tools(
        &self,
        session: &SessionRow,
        turn: &TurnRow,
        new_session: bool,
        tools: &[ToolCallRow],
    ) -> Result<()> {
        self.append_turn_with_activity(session, turn, new_session, tools, &[])
    }

    pub fn append_turn_with_activity(
        &self,
        session: &SessionRow,
        turn: &TurnRow,
        new_session: bool,
        tools: &[ToolCallRow],
        reasoning: &[ReasoningSummaryRow],
    ) -> Result<()> {
        self.append_turn_with_full_activity(session, turn, new_session, tools, reasoning, &[])
    }

    pub fn append_turn_with_full_activity(
        &self,
        session: &SessionRow,
        turn: &TurnRow,
        new_session: bool,
        tools: &[ToolCallRow],
        reasoning: &[ReasoningSummaryRow],
        commentary: &[CommentaryRow],
    ) -> Result<()> {
        self.with_connection(|connection| {
            let transaction = connection.transaction()?;
            if new_session {
                transaction.execute(
                    "INSERT INTO sessions(id, workspace_id, title, created_at_ms, last_message_at_ms) VALUES (?1, ?2, ?3, ?4, ?5)",
                    params![session.id, session.workspace_id, session.title, session.created_at_ms, session.last_message_at_ms],
                )?;
            }
            transaction.execute(
                "INSERT INTO session_turns(session_id, number, provider, method, model, prompt, answer, duration_ms)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
                params![session.id, turn.number, turn.provider, turn.method, turn.model, turn.prompt, turn.answer, turn.duration_ms],
            )?;
            if !new_session {
                transaction.execute(
                    "UPDATE sessions SET last_message_at_ms = ?2 WHERE id = ?1",
                    params![session.id, session.last_message_at_ms],
                )?;
            }
            for tool in tools {
                transaction.execute(
                    "INSERT INTO session_tool_calls(id, session_id, turn_number, ordinal, name, summary, arguments, output, is_error)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
                    params![tool.id, session.id, turn.number, tool.ordinal, tool.name, tool.summary, tool.arguments, tool.output, tool.is_error],
                )?;
            }
            for item in reasoning {
                transaction.execute(
                    "INSERT INTO session_reasoning_summaries(session_id, turn_number, ordinal, after_tool_count, text)
                     VALUES (?1, ?2, ?3, ?4, ?5)",
                    params![session.id, turn.number, item.ordinal, item.after_tool_count, item.text],
                )?;
            }
            for item in commentary {
                transaction.execute(
                    "INSERT INTO session_commentary(session_id, turn_number, ordinal, after_tool_count, text)
                     VALUES (?1, ?2, ?3, ?4, ?5)",
                    params![session.id, turn.number, item.ordinal, item.after_tool_count, item.text],
                )?;
            }
            transaction.commit()?;
            Ok(())
        })
    }

    pub fn tool_summaries(&self, session_id: &str) -> Result<Vec<ToolCallSummaryRow>> {
        self.with_connection(|connection| {
            let mut statement = connection.prepare(
                "SELECT id, turn_number, name, summary, is_error FROM session_tool_calls
                 WHERE session_id = ?1 ORDER BY turn_number, ordinal LIMIT 1025",
            )?;
            let rows = statement.query_map([session_id], |row| {
                Ok(ToolCallSummaryRow {
                    id: row.get(0)?,
                    turn_number: row.get(1)?,
                    name: row.get(2)?,
                    summary: row.get(3)?,
                    is_error: row.get(4)?,
                })
            })?;
            rows.collect::<rusqlite::Result<Vec<_>>>()
                .map_err(Into::into)
        })
    }

    pub fn reasoning_summaries(&self, session_id: &str) -> Result<Vec<ReasoningSummaryRow>> {
        self.with_connection(|connection| {
            let mut statement = connection.prepare(
                "SELECT turn_number, ordinal, after_tool_count, text FROM session_reasoning_summaries
                 WHERE session_id = ?1 ORDER BY turn_number, ordinal LIMIT 1025",
            )?;
            let rows = statement.query_map([session_id], |row| {
                Ok(ReasoningSummaryRow {
                    turn_number: row.get(0)?,
                    ordinal: row.get(1)?,
                    after_tool_count: row.get(2)?,
                    text: row.get(3)?,
                })
            })?;
            rows.collect::<rusqlite::Result<Vec<_>>>()
                .map_err(Into::into)
        })
    }

    pub fn commentary(&self, session_id: &str) -> Result<Vec<CommentaryRow>> {
        self.with_connection(|connection| {
            let mut statement = connection.prepare(
                "SELECT turn_number, ordinal, after_tool_count, text FROM session_commentary
                 WHERE session_id = ?1 ORDER BY turn_number, ordinal LIMIT 1025",
            )?;
            let rows = statement.query_map([session_id], |row| {
                Ok(CommentaryRow {
                    turn_number: row.get(0)?,
                    ordinal: row.get(1)?,
                    after_tool_count: row.get(2)?,
                    text: row.get(3)?,
                })
            })?;
            rows.collect::<rusqlite::Result<Vec<_>>>()
                .map_err(Into::into)
        })
    }

    pub fn tool_call(&self, session_id: &str, id: &str) -> Result<Option<ToolCallRow>> {
        self.with_connection(|connection| {
            connection
                .query_row(
                    "SELECT id, turn_number, ordinal, name, summary, arguments, output, is_error
                 FROM session_tool_calls WHERE session_id = ?1 AND id = ?2",
                    params![session_id, id],
                    |row| {
                        Ok(ToolCallRow {
                            id: row.get(0)?,
                            turn_number: row.get(1)?,
                            ordinal: row.get(2)?,
                            name: row.get(3)?,
                            summary: row.get(4)?,
                            arguments: row.get(5)?,
                            output: row.get(6)?,
                            is_error: row.get(7)?,
                        })
                    },
                )
                .optional()
                .map_err(Into::into)
        })
    }

    pub fn setting(&self, key: &str) -> Result<Option<String>> {
        self.with_connection(|connection| {
            Ok(connection
                .query_row(
                    "SELECT value FROM provider_settings WHERE key = ?1",
                    [key],
                    |row| row.get(0),
                )
                .optional()?)
        })
    }

    pub fn permission_rule(&self, id: &str) -> Result<Option<(String, String, bool)>> {
        self.with_connection(|connection| {
            Ok(connection
                .query_row(
                    "SELECT title, instruction, enabled FROM permission_rules WHERE id = ?1",
                    [id],
                    |row| Ok((row.get(0)?, row.get(1)?, row.get::<_, i64>(2)? != 0)),
                )
                .optional()?)
        })
    }

    pub fn custom_permission_rules(&self) -> Result<Vec<CustomPermissionRuleRow>> {
        self.with_connection(|connection| {
            let mut statement = connection.prepare(
                "SELECT id, stage, title, instruction, enabled FROM permission_rules
                 WHERE stage IS NOT NULL ORDER BY stage, position, id",
            )?;
            let rows = statement.query_map([], |row| {
                Ok(CustomPermissionRuleRow {
                    id: row.get(0)?,
                    stage: row.get(1)?,
                    title: row.get(2)?,
                    instruction: row.get(3)?,
                    enabled: row.get::<_, i64>(4)? != 0,
                })
            })?;
            rows.collect::<rusqlite::Result<Vec<_>>>()
                .map_err(Into::into)
        })
    }

    pub fn create_custom_permission_rule(&self, rule: &CustomPermissionRuleRow) -> Result<()> {
        self.with_connection(|connection| {
            let transaction = connection.transaction()?;
            let count: i64 = transaction.query_row(
                "SELECT COUNT(*) FROM permission_rules WHERE stage IS NOT NULL",
                [],
                |row| row.get(0),
            )?;
            ensure!(count < 11, "permission rule catalog is full");
            let position: i64 = transaction.query_row(
                "SELECT COALESCE(MAX(position), 0) + 1 FROM permission_rules WHERE stage = ?1",
                [rule.stage],
                |row| row.get(0),
            )?;
            transaction.execute(
                "INSERT INTO permission_rules(id, title, instruction, enabled, stage, position)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                params![
                    rule.id,
                    rule.title,
                    rule.instruction,
                    i64::from(rule.enabled),
                    rule.stage,
                    position
                ],
            )?;
            transaction.commit()?;
            Ok(())
        })
    }

    pub fn save_permission_rule(
        &self,
        id: &str,
        title: &str,
        instruction: &str,
        enabled: bool,
    ) -> Result<()> {
        self.with_connection(|connection| {
            connection.execute(
                "INSERT INTO permission_rules(id, title, instruction, enabled) VALUES (?1, ?2, ?3, ?4)
                 ON CONFLICT(id) DO UPDATE SET title = excluded.title, instruction = excluded.instruction, enabled = excluded.enabled",
                params![id, title, instruction, i64::from(enabled)],
            )?;
            Ok(())
        })
    }

    pub fn save_setting(&self, key: &str, value: Option<&str>) -> Result<()> {
        self.with_connection(|connection| {
            if let Some(value) = value {
                connection.execute(
                    "INSERT INTO provider_settings(key, value) VALUES (?1, ?2)
                     ON CONFLICT(key) DO UPDATE SET value = excluded.value",
                    params![key, value],
                )?;
            } else {
                connection.execute("DELETE FROM provider_settings WHERE key = ?1", [key])?;
            }
            Ok(())
        })
    }
}

fn session_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<SessionRow> {
    Ok(SessionRow {
        id: row.get(0)?,
        workspace_id: row.get(1)?,
        title: row.get(2)?,
        created_at_ms: row.get(3)?,
        last_message_at_ms: row.get(4)?,
    })
}

fn private_root(root: &Path) -> Result<()> {
    std::fs::create_dir_all(root).context("create private host data directory")?;
    #[cfg(windows)]
    restrict_windows_acl(root, true)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(root, std::fs::Permissions::from_mode(0o700))
            .context("restrict host data directory")?;
    }
    Ok(())
}

#[cfg(windows)]
fn restrict_windows_acl(path: &Path, directory: bool) -> Result<()> {
    use std::process::Command;
    let user = std::env::var("USERNAME").context("Windows user is unavailable")?;
    let domain = std::env::var("USERDOMAIN").context("Windows domain is unavailable")?;
    let principal = format!("{domain}\\{user}");
    let permission = if directory {
        format!("{principal}:(OI)(CI)F")
    } else {
        format!("{principal}:F")
    };
    let status = Command::new("icacls")
        .arg(path)
        .arg("/inheritance:r")
        .arg("/grant:r")
        .arg(permission)
        .output()
        .context("cannot set private Windows ACL")?;
    if !status.status.success() {
        bail!("cannot set private Windows ACL");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn root() -> std::path::PathBuf {
        std::env::temp_dir().join(format!("beale-storage-example-{}", uuid::Uuid::new_v4()))
    }

    #[test]
    fn persists_workspace_and_provider_state() {
        let root = root();
        let db = HostDatabase::open(&root).unwrap();
        let workspace = WorkspaceRow {
            id: "workspace-example".to_owned(),
            name: "Example Workspace".to_owned(),
            directory: "example-directory".to_owned(),
        };
        db.replace_workspaces(std::slice::from_ref(&workspace))
            .unwrap();
        assert_eq!(db.workspaces().unwrap(), [workspace]);
        db.save_setting("lead-provider", Some("xai")).unwrap();
        drop(db);
        let reopened = HostDatabase::open(&root).unwrap();
        assert_eq!(reopened.workspaces().unwrap().len(), 1);
        assert_eq!(
            reopened.setting("lead-provider").unwrap().as_deref(),
            Some("xai")
        );
        drop(reopened);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn appends_session_turns_atomically() {
        let root = root();
        let db = HostDatabase::open(&root).unwrap();
        let session = SessionRow {
            id: "session-example".to_owned(),
            workspace_id: "workspace-example".to_owned(),
            title: "Example prompt".to_owned(),
            created_at_ms: 1,
            last_message_at_ms: 1,
        };
        let first = TurnRow {
            number: 1,
            provider: 1,
            method: 1,
            model: "model-example".to_owned(),
            prompt: "Example prompt".to_owned(),
            answer: "Example answer".to_owned(),
            duration_ms: 1_234,
        };
        db.append_turn(&session, &first, true).unwrap();
        assert!(db.append_turn(&session, &first, false).is_err());
        let second = TurnRow {
            number: 2,
            ..first.clone()
        };
        let updated_session = SessionRow {
            last_message_at_ms: 42,
            ..session.clone()
        };
        let reasoning = ReasoningSummaryRow {
            turn_number: 2,
            ordinal: 0,
            after_tool_count: 0,
            text: "Checked the example input.".to_owned(),
        };
        db.append_turn_with_activity(
            &updated_session,
            &second,
            false,
            &[],
            std::slice::from_ref(&reasoning),
        )
        .unwrap();
        assert_eq!(db.reasoning_summaries(&session.id).unwrap(), [reasoning]);
        assert_eq!(
            db.session(&session.id).unwrap().unwrap().turns,
            [first, second]
        );
        assert_eq!(db.sessions().unwrap(), [updated_session]);
        drop(db);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn session_listing_excludes_internal_quick_chats() {
        let root = root();
        let db = HostDatabase::open(&root).unwrap();
        for (id, workspace_id) in [
            ("research-session-example", "workspace-example"),
            ("quick-chat-example", "internal:quick-chats"),
        ] {
            let session = SessionRow {
                id: id.to_owned(),
                workspace_id: workspace_id.to_owned(),
                title: "Example".to_owned(),
                created_at_ms: 1,
                last_message_at_ms: 1,
            };
            let turn = TurnRow {
                number: 1,
                provider: 1,
                method: 1,
                model: "model-example".to_owned(),
                prompt: "Example question".to_owned(),
                answer: "Example answer".to_owned(),
                duration_ms: 1,
            };
            if workspace_id == "internal:quick-chats" {
                db.append_new_quick_chat_turn(&session, &turn, "workspace-example")
                    .unwrap();
            } else {
                db.append_turn(&session, &turn, true).unwrap();
            }
        }
        let visible = db.sessions_excluding("internal:quick-chats").unwrap();
        assert_eq!(visible.len(), 1);
        assert_eq!(visible[0].id, "research-session-example");
        assert_eq!(
            db.archived_quick_chat_ids().unwrap(),
            ["quick-chat-example"]
        );
        assert!(db.archive_session("research-session-example", 2).unwrap());
        assert!(
            db.sessions_excluding("internal:quick-chats")
                .unwrap()
                .is_empty()
        );
        assert!(db.session("research-session-example").unwrap().is_some());
        let archive = db.archive_items().unwrap();
        assert_eq!(archive.len(), 2);
        assert_eq!(archive[0].id, "research-session-example");
        assert_eq!(archive[0].model, "model-example");
        assert_eq!(archive[0].turn_count, 1);
        assert!(!archive[0].quick_chat);
        assert!(archive[1].quick_chat);
        assert_eq!(archive[1].workspace_id, "workspace-example");
        assert!(db.restore_session("research-session-example").unwrap());
        assert!(!db.restore_session("research-session-example").unwrap());
        assert!(!db.restore_session("quick-chat-example").unwrap());
        assert_eq!(
            db.sessions_excluding("internal:quick-chats").unwrap().len(),
            1
        );
        assert_eq!(db.archive_items().unwrap().len(), 1);
        assert_eq!(
            db.quick_chat_workspace("quick-chat-example")
                .unwrap()
                .as_deref(),
            Some("workspace-example")
        );
        drop(db);
        let reopened = HostDatabase::open(&root).unwrap();
        assert_eq!(
            reopened
                .quick_chat_workspace("quick-chat-example")
                .unwrap()
                .as_deref(),
            Some("workspace-example")
        );
        drop(reopened);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn schema_six_migration_binds_existing_quick_chats_to_no_workspace() {
        let root = root();
        let db = HostDatabase::open(&root).unwrap();
        db.append_turn(
            &SessionRow {
                id: "old-quick-chat-example".to_owned(),
                workspace_id: "internal:quick-chats".to_owned(),
                title: "Example".to_owned(),
                created_at_ms: 1,
                last_message_at_ms: 1,
            },
            &TurnRow {
                number: 1,
                provider: 1,
                method: 1,
                model: "model-example".to_owned(),
                prompt: "Example question".to_owned(),
                answer: "Example answer".to_owned(),
                duration_ms: 1,
            },
            true,
        )
        .unwrap();
        drop(db);
        let connection = Connection::open(root.join("state.sqlite")).unwrap();
        connection
            .execute_batch(
                "DROP TABLE plugin_installs;
                 DROP TABLE quick_chat_contexts;
                 ALTER TABLE sessions DROP COLUMN last_message_at_ms;
                 ALTER TABLE sessions DROP COLUMN archived_at_ms;
                 ALTER TABLE permission_rules DROP COLUMN title;
                 ALTER TABLE permission_rules DROP COLUMN stage;
                 ALTER TABLE permission_rules DROP COLUMN position;
                 PRAGMA user_version = 5;",
            )
            .unwrap();
        drop(connection);
        let migrated = HostDatabase::open(&root).unwrap();
        assert_eq!(
            migrated
                .quick_chat_workspace("old-quick-chat-example")
                .unwrap(),
            Some(String::new())
        );
        assert_eq!(
            migrated.archived_quick_chat_ids().unwrap(),
            ["old-quick-chat-example"]
        );
        drop(migrated);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn refuses_a_newer_schema_without_modifying_it() {
        let root = root();
        let db = HostDatabase::open(&root).unwrap();
        drop(db);
        let connection = Connection::open(root.join("state.sqlite")).unwrap();
        connection
            .execute_batch("PRAGMA user_version = 13;")
            .unwrap();
        drop(connection);
        assert!(HostDatabase::open(&root).is_err());
        let connection = Connection::open(root.join("state.sqlite")).unwrap();
        let version: i64 = connection
            .query_row("PRAGMA user_version", [], |row| row.get(0))
            .unwrap();
        assert_eq!(version, 13);
        drop(connection);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn schema_eleven_adds_plugin_install_registry() {
        let root = root();
        let db = HostDatabase::open(&root).unwrap();
        db.save_permission_rule("auto-scope", "Scope", "Example instruction", true)
            .unwrap();
        drop(db);
        let connection = Connection::open(root.join("state.sqlite")).unwrap();
        connection
            .execute_batch("DROP TABLE plugin_installs; ALTER TABLE sessions DROP COLUMN last_message_at_ms; PRAGMA user_version = 10;")
            .unwrap();
        drop(connection);
        let migrated = HostDatabase::open(&root).unwrap();
        assert!(migrated.installed_plugin_names().unwrap().is_empty());
        assert!(migrated.install_plugin("example-plugin", 123).unwrap());
        assert!(!migrated.install_plugin("example-plugin", 456).unwrap());
        assert_eq!(
            migrated.installed_plugin_names().unwrap(),
            ["example-plugin"]
        );
        assert_eq!(
            migrated.permission_rule("auto-scope").unwrap().unwrap().0,
            "Scope"
        );
        drop(migrated);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn migrates_permission_rule_titles_from_schema_eight() {
        let root = root();
        let db = HostDatabase::open(&root).unwrap();
        db.save_permission_rule("auto-scope", "Example title", "Example instruction", false)
            .unwrap();
        drop(db);
        let connection = Connection::open(root.join("state.sqlite")).unwrap();
        connection
            .execute_batch(
                "DROP TABLE plugin_installs;
                 ALTER TABLE sessions DROP COLUMN last_message_at_ms;
                 ALTER TABLE permission_rules DROP COLUMN title;
                 ALTER TABLE permission_rules DROP COLUMN stage;
                 ALTER TABLE permission_rules DROP COLUMN position;
                 PRAGMA user_version = 8;",
            )
            .unwrap();
        drop(connection);
        let db = HostDatabase::open(&root).unwrap();
        assert_eq!(
            db.permission_rule("auto-scope").unwrap(),
            Some((String::new(), "Example instruction".to_owned(), false))
        );
        db.save_permission_rule("auto-scope", "Updated title", "Example instruction", true)
            .unwrap();
        assert_eq!(
            db.permission_rule("auto-scope").unwrap(),
            Some((
                "Updated title".to_owned(),
                "Example instruction".to_owned(),
                true
            ))
        );
        drop(db);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn schema_ten_adds_custom_rules_without_changing_existing_rules() {
        let root = root();
        let db = HostDatabase::open(&root).unwrap();
        db.save_permission_rule("auto-scope", "Scope", "Example scope instruction", false)
            .unwrap();
        drop(db);
        let connection = Connection::open(root.join("state.sqlite")).unwrap();
        connection
            .execute_batch(
                "DROP TABLE plugin_installs;
                 ALTER TABLE sessions DROP COLUMN last_message_at_ms;
                 ALTER TABLE permission_rules DROP COLUMN stage;
                 ALTER TABLE permission_rules DROP COLUMN position;
                 PRAGMA user_version = 9;",
            )
            .unwrap();
        drop(connection);
        let db = HostDatabase::open(&root).unwrap();
        assert_eq!(
            db.permission_rule("auto-scope").unwrap(),
            Some((
                "Scope".to_owned(),
                "Example scope instruction".to_owned(),
                false
            ))
        );
        let custom = CustomPermissionRuleRow {
            id: "custom-example".to_owned(),
            stage: 1,
            title: "Example rule".to_owned(),
            instruction: "Review example operations.".to_owned(),
            enabled: true,
        };
        db.create_custom_permission_rule(&custom).unwrap();
        drop(db);
        let reopened = HostDatabase::open(&root).unwrap();
        assert_eq!(reopened.custom_permission_rules().unwrap(), vec![custom]);
        drop(reopened);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn migrates_existing_sessions_to_tool_call_schema() {
        let root = root();
        std::fs::create_dir_all(&root).unwrap();
        let connection = Connection::open(root.join("state.sqlite")).unwrap();
        connection.execute_batch("CREATE TABLE sessions (id TEXT PRIMARY KEY, workspace_id TEXT NOT NULL, title TEXT NOT NULL, created_at_ms INTEGER NOT NULL);
            CREATE TABLE session_turns (session_id TEXT NOT NULL REFERENCES sessions(id) ON DELETE CASCADE, number INTEGER NOT NULL, provider INTEGER NOT NULL, method INTEGER NOT NULL, model TEXT NOT NULL, prompt TEXT NOT NULL, answer TEXT NOT NULL, PRIMARY KEY(session_id, number));
            INSERT INTO sessions VALUES ('session-example','workspace-example','Example',1);
            INSERT INTO session_turns VALUES ('session-example',1,1,1,'model-example','Hello','Hello');
            PRAGMA user_version = 1;").unwrap();
        drop(connection);
        let db = HostDatabase::open(&root).unwrap();
        assert_eq!(
            db.session("session-example").unwrap().unwrap().turns.len(),
            1
        );
        assert_eq!(db.sessions().unwrap()[0].last_message_at_ms, 1);
        assert!(db.tool_summaries("session-example").unwrap().is_empty());
        assert!(
            db.reasoning_summaries("session-example")
                .unwrap()
                .is_empty()
        );
        drop(db);
        std::fs::remove_dir_all(root).unwrap();
    }
}
