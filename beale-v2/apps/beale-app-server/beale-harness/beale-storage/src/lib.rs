use anyhow::{Context, Result, bail};
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
pub struct SessionRow {
    pub id: String,
    pub workspace_id: String,
    pub title: String,
    pub created_at_ms: i64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TurnRow {
    pub number: i64,
    pub provider: i32,
    pub method: i32,
    pub model: String,
    pub prompt: String,
    pub answer: String,
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
        if version > 2 {
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
        Ok(Self(Arc::new(Mutex::new(connection))))
    }

    fn with_connection<T>(&self, action: impl FnOnce(&mut Connection) -> Result<T>) -> Result<T> {
        let mut connection = self
            .0
            .lock()
            .map_err(|_| anyhow::anyhow!("host database lock poisoned"))?;
        action(&mut connection)
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
                "SELECT id, workspace_id, title, created_at_ms FROM sessions
                 ORDER BY created_at_ms DESC LIMIT 1025",
            )?;
            let rows = statement.query_map([], session_from_row)?;
            rows.collect::<rusqlite::Result<Vec<_>>>()
                .map_err(Into::into)
        })
    }

    pub fn session(&self, id: &str) -> Result<Option<SessionWithTurns>> {
        self.with_connection(|connection| {
            let session = connection
                .query_row(
                    "SELECT id, workspace_id, title, created_at_ms FROM sessions WHERE id = ?1",
                    [id],
                    session_from_row,
                )
                .optional()?;
            let Some(session) = session else {
                return Ok(None);
            };
            let mut statement = connection.prepare(
                "SELECT number, provider, method, model, prompt, answer FROM session_turns
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

    pub fn append_turn_with_tools(
        &self,
        session: &SessionRow,
        turn: &TurnRow,
        new_session: bool,
        tools: &[ToolCallRow],
    ) -> Result<()> {
        self.with_connection(|connection| {
            let transaction = connection.transaction()?;
            if new_session {
                transaction.execute(
                    "INSERT INTO sessions(id, workspace_id, title, created_at_ms) VALUES (?1, ?2, ?3, ?4)",
                    params![session.id, session.workspace_id, session.title, session.created_at_ms],
                )?;
            }
            transaction.execute(
                "INSERT INTO session_turns(session_id, number, provider, method, model, prompt, answer)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                params![session.id, turn.number, turn.provider, turn.method, turn.model, turn.prompt, turn.answer],
            )?;
            for tool in tools {
                transaction.execute(
                    "INSERT INTO session_tool_calls(id, session_id, turn_number, ordinal, name, summary, arguments, output, is_error)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
                    params![tool.id, session.id, turn.number, tool.ordinal, tool.name, tool.summary, tool.arguments, tool.output, tool.is_error],
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
        };
        let first = TurnRow {
            number: 1,
            provider: 1,
            method: 1,
            model: "model-example".to_owned(),
            prompt: "Example prompt".to_owned(),
            answer: "Example answer".to_owned(),
        };
        db.append_turn(&session, &first, true).unwrap();
        assert!(db.append_turn(&session, &first, false).is_err());
        let second = TurnRow {
            number: 2,
            ..first.clone()
        };
        db.append_turn(&session, &second, false).unwrap();
        assert_eq!(
            db.session(&session.id).unwrap().unwrap().turns,
            [first, second]
        );
        assert_eq!(db.sessions().unwrap(), [session]);
        drop(db);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn refuses_a_newer_schema_without_modifying_it() {
        let root = root();
        let db = HostDatabase::open(&root).unwrap();
        drop(db);
        let connection = Connection::open(root.join("state.sqlite")).unwrap();
        connection
            .execute_batch("PRAGMA user_version = 3;")
            .unwrap();
        drop(connection);
        assert!(HostDatabase::open(&root).is_err());
        let connection = Connection::open(root.join("state.sqlite")).unwrap();
        let version: i64 = connection
            .query_row("PRAGMA user_version", [], |row| row.get(0))
            .unwrap();
        assert_eq!(version, 3);
        drop(connection);
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
        assert!(db.tool_summaries("session-example").unwrap().is_empty());
        drop(db);
        std::fs::remove_dir_all(root).unwrap();
    }
}
