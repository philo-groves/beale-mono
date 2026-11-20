//! Graphs persistence in the shared host database.
//!
//! This feature owns these tables and applies [`MIGRATIONS`] under its feature
//! ID; builds without the feature never create them.

use anyhow::{Result, ensure};
use beale_storage::{
    AutomationOptions, CommittedTurn, HostDatabase, SessionRow, TurnExtension,
    rusqlite::{self, OptionalExtension, Transaction, params},
};

/// Ordered schema versions for this feature's tables.
pub const MIGRATIONS: &[&str] = &[V1];

const V1: &str = "
CREATE TABLE session_graphs (
    session_id TEXT PRIMARY KEY REFERENCES sessions(id) ON DELETE CASCADE,
    graph_id TEXT NOT NULL,
    config_blob BLOB NOT NULL DEFAULT X''
);
CREATE TABLE session_graph_turns (
    session_id TEXT NOT NULL REFERENCES sessions(id) ON DELETE CASCADE,
    turn_number INTEGER NOT NULL,
    agent_role TEXT NOT NULL,
    status TEXT NOT NULL CHECK(status IN ('running','completed','failed')),
    PRIMARY KEY(session_id, turn_number)
);
CREATE TABLE session_graph_handoffs (
    id TEXT PRIMARY KEY,
    session_id TEXT NOT NULL REFERENCES sessions(id) ON DELETE CASCADE,
    from_role TEXT NOT NULL,
    to_role TEXT NOT NULL,
    task TEXT NOT NULL,
    status TEXT NOT NULL CHECK(status IN ('pending','running','completed','failed')),
    created_at_ms INTEGER NOT NULL
);
CREATE INDEX session_graph_handoffs_queue ON session_graph_handoffs(session_id, status, to_role, created_at_ms);
";

/// Creates or upgrades this feature's tables.
pub fn migrate(db: &HostDatabase) -> Result<()> {
    db.migrate_schema(crate::FEATURE_ID, MIGRATIONS)
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GraphHandoffRow {
    pub id: String,
    pub from_role: String,
    pub to_role: String,
    pub task: String,
    pub status: String,
}

/// Graph configuration, peer turn reservations, and handoffs on the host database.
pub trait GraphStore {
    fn session_graph_id(&self, session_id: &str) -> Result<Option<String>>;
    fn session_graph_config(&self, session_id: &str) -> Result<Option<Vec<u8>>>;
    fn create_peer_graph_session(
        &self,
        session: &SessionRow,
        graph_id: &str,
        graph_config: &[u8],
        repeat: Option<(&str, AutomationOptions)>,
    ) -> Result<()>;
    fn is_peer_graph_session(&self, session_id: &str) -> Result<bool>;
    fn reserve_graph_turn(&self, session_id: &str, role: &str) -> Result<i64>;
    fn graph_turn_role(&self, session_id: &str, turn_number: i64) -> Result<Option<String>>;
    fn graph_role_status(&self, session_id: &str, role: &str) -> Result<String>;
    fn finish_graph_turn(&self, session_id: &str, turn_number: i64, success: bool) -> Result<()>;
    fn enqueue_graph_handoff(
        &self,
        session_id: &str,
        item: &GraphHandoffRow,
        created_at_ms: i64,
    ) -> Result<()>;
    fn graph_handoffs(&self, session_id: &str) -> Result<Vec<GraphHandoffRow>>;
    fn pending_graph_handoffs(&self, session_id: &str) -> Result<Vec<GraphHandoffRow>>;
    fn set_graph_handoff_status(&self, id: &str, status: &str) -> Result<()>;
    fn recover_graph_session(&self, session_id: &str) -> Result<()>;
    fn recover_abandoned_graph_work(&self) -> Result<()>;
}

impl GraphStore for HostDatabase {
    fn session_graph_id(&self, session_id: &str) -> Result<Option<String>> {
        self.with_connection(|connection| {
            connection
                .query_row(
                    "SELECT graph_id FROM session_graphs WHERE session_id = ?1",
                    [session_id],
                    |row| row.get(0),
                )
                .optional()
                .map_err(Into::into)
        })
    }

    fn session_graph_config(&self, session_id: &str) -> Result<Option<Vec<u8>>> {
        self.with_connection(|connection| {
            connection
                .query_row(
                    "SELECT config_blob FROM session_graphs WHERE session_id = ?1",
                    [session_id],
                    |row| row.get(0),
                )
                .optional()
                .map_err(Into::into)
        })
    }

    fn create_peer_graph_session(
        &self,
        session: &SessionRow,
        graph_id: &str,
        graph_config: &[u8],
        repeat: Option<(&str, AutomationOptions)>,
    ) -> Result<()> {
        ensure!(
            !graph_id.is_empty() && graph_id.len() <= 64,
            "invalid graph id"
        );
        ensure!(graph_config.len() <= 32_768, "graph config is too large");
        self.create_session(session, repeat, |transaction| {
            transaction.execute(
                "INSERT INTO session_graphs(session_id,graph_id,config_blob) VALUES (?1,?2,?3)",
                params![session.id, graph_id, graph_config],
            )?;
            Ok(())
        })
    }

    fn is_peer_graph_session(&self, session_id: &str) -> Result<bool> {
        self.with_connection(|connection| {
            connection
                .query_row(
                    "SELECT EXISTS(SELECT 1 FROM session_graph_turns WHERE session_id=?1)
                    OR EXISTS(SELECT 1 FROM session_graph_handoffs WHERE session_id=?1)
                    OR EXISTS(SELECT 1 FROM session_graphs WHERE session_id=?1
                        AND graph_id IN ('cloudflare_harness','openai_pipeline','sequential_pipeline')
                        AND NOT EXISTS(SELECT 1 FROM session_turns WHERE session_id=?1))",
                    [session_id],
                    |row| row.get(0),
                )
                .map_err(Into::into)
        })
    }

    fn reserve_graph_turn(&self, session_id: &str, role: &str) -> Result<i64> {
        ensure!(!role.is_empty() && role.len() <= 20, "invalid graph role");
        self.with_connection(|connection| {
            let transaction = connection.transaction()?;
            let next: i64 = transaction.query_row(
                "SELECT MAX(number)+1 FROM (
                    SELECT number FROM session_turns WHERE session_id=?1
                    UNION ALL SELECT turn_number AS number FROM session_graph_turns WHERE session_id=?1
                 )",
                [session_id],
                |row| row.get::<_, Option<i64>>(0),
            )?.unwrap_or(1);
            ensure!((1..=1024).contains(&next), "graph turn limit reached");
            transaction.execute(
                "INSERT INTO session_graph_turns(session_id,turn_number,agent_role,status)
                 VALUES (?1,?2,?3,'running')",
                params![session_id,next,role],
            )?;
            transaction.commit()?;
            Ok(next)
        })
    }

    fn graph_turn_role(&self, session_id: &str, turn_number: i64) -> Result<Option<String>> {
        self.with_connection(|connection| {
            connection.query_row(
                "SELECT agent_role FROM session_graph_turns WHERE session_id=?1 AND turn_number=?2",
                params![session_id,turn_number], |row| row.get(0),
            ).optional().map_err(Into::into)
        })
    }

    fn graph_role_status(&self, session_id: &str, role: &str) -> Result<String> {
        self.with_connection(|connection| {
            let running: bool = connection.query_row(
                "SELECT EXISTS(SELECT 1 FROM session_graph_turns
                 WHERE session_id=?1 AND agent_role=?2 AND status='running')",
                params![session_id, role],
                |row| row.get(0),
            )?;
            if running {
                return Ok("working".to_owned());
            }
            let pending: bool = connection.query_row(
                "SELECT EXISTS(SELECT 1 FROM session_graph_handoffs
                 WHERE session_id=?1 AND to_role=?2 AND status='pending')",
                params![session_id, role],
                |row| row.get(0),
            )?;
            if pending {
                return Ok("queued".to_owned());
            }
            let failed: bool = connection.query_row(
                "SELECT COALESCE((SELECT status='failed' FROM session_graph_turns
                 WHERE session_id=?1 AND agent_role=?2 ORDER BY turn_number DESC LIMIT 1),0)",
                params![session_id, role],
                |row| row.get(0),
            )?;
            Ok(if failed { "failed" } else { "idle" }.to_owned())
        })
    }

    fn finish_graph_turn(&self, session_id: &str, turn_number: i64, success: bool) -> Result<()> {
        self.with_connection(|connection| {
            connection.execute(
                "UPDATE session_graph_turns SET status=?3 WHERE session_id=?1 AND turn_number=?2",
                params![
                    session_id,
                    turn_number,
                    if success { "completed" } else { "failed" }
                ],
            )?;
            Ok(())
        })
    }

    fn enqueue_graph_handoff(
        &self,
        session_id: &str,
        item: &GraphHandoffRow,
        created_at_ms: i64,
    ) -> Result<()> {
        ensure!(
            !item.task.trim().is_empty() && item.task.len() <= 4096,
            "invalid handoff task"
        );
        self.with_connection(|connection| {
            connection.execute(
                "INSERT INTO session_graph_handoffs(id,session_id,from_role,to_role,task,status,created_at_ms)
                 VALUES (?1,?2,?3,?4,?5,'pending',?6)",
                params![item.id,session_id,item.from_role,item.to_role,item.task,created_at_ms],
            )?;
            Ok(())
        })
    }

    fn graph_handoffs(&self, session_id: &str) -> Result<Vec<GraphHandoffRow>> {
        self.with_connection(|connection| {
            let mut query = connection.prepare(
                "SELECT id,from_role,to_role,task,status FROM session_graph_handoffs
                 WHERE session_id=?1 ORDER BY created_at_ms DESC,id DESC LIMIT 128",
            )?;
            query
                .query_map([session_id], |row| {
                    Ok(GraphHandoffRow {
                        id: row.get(0)?,
                        from_role: row.get(1)?,
                        to_role: row.get(2)?,
                        task: row.get(3)?,
                        status: row.get(4)?,
                    })
                })?
                .collect::<rusqlite::Result<Vec<_>>>()
                .map_err(Into::into)
        })
    }

    fn pending_graph_handoffs(&self, session_id: &str) -> Result<Vec<GraphHandoffRow>> {
        self.with_connection(|connection| {
            let mut query = connection.prepare(
                "SELECT id,from_role,to_role,task,status FROM session_graph_handoffs
                 WHERE session_id=?1 AND status='pending' ORDER BY created_at_ms,id LIMIT 128",
            )?;
            query
                .query_map([session_id], |row| {
                    Ok(GraphHandoffRow {
                        id: row.get(0)?,
                        from_role: row.get(1)?,
                        to_role: row.get(2)?,
                        task: row.get(3)?,
                        status: row.get(4)?,
                    })
                })?
                .collect::<rusqlite::Result<Vec<_>>>()
                .map_err(Into::into)
        })
    }

    fn set_graph_handoff_status(&self, id: &str, status: &str) -> Result<()> {
        ensure!(
            matches!(status, "pending" | "running" | "completed" | "failed"),
            "invalid handoff status"
        );
        self.with_connection(|connection| {
            connection.execute(
                "UPDATE session_graph_handoffs SET status=?2 WHERE id=?1",
                params![id, status],
            )?;
            Ok(())
        })
    }

    fn recover_graph_session(&self, session_id: &str) -> Result<()> {
        self.with_connection(|connection| {
            let transaction = connection.transaction()?;
            transaction.execute(
                "UPDATE session_graph_turns SET status='failed'
                 WHERE session_id=?1 AND status='running'",
                [session_id],
            )?;
            transaction.execute(
                "UPDATE session_graph_handoffs SET status='pending'
                 WHERE session_id=?1 AND status='running'",
                [session_id],
            )?;
            transaction.commit()?;
            Ok(())
        })
    }

    fn recover_abandoned_graph_work(&self) -> Result<()> {
        self.with_connection(|connection| {
            let transaction = connection.transaction()?;
            transaction.execute(
                "UPDATE session_graph_turns SET status='failed' WHERE status='running'",
                [],
            )?;
            transaction.execute(
                "UPDATE session_graph_handoffs SET status='pending' WHERE status='running'",
                [],
            )?;
            transaction.commit()?;
            Ok(())
        })
    }
}

/// Records a session's graph selection with its first turn and completes a
/// reserved peer agent turn.
pub struct GraphCommit<'a> {
    /// Graph ID and encoded configuration saved with a new session.
    pub selection: Option<(&'a str, &'a [u8])>,
}

impl TurnExtension for GraphCommit<'_> {
    fn commit(&self, transaction: &Transaction<'_>, turn: &CommittedTurn<'_>) -> Result<()> {
        if let Some((graph_id, config)) = self.selection {
            ensure!(
                turn.new_session && !graph_id.is_empty() && graph_id.len() <= 64,
                "graph requires a new session"
            );
            ensure!(config.len() <= 32_768, "graph config must be bounded");
            transaction.execute(
                "INSERT INTO session_graphs(session_id, graph_id, config_blob) VALUES (?1, ?2, ?3)",
                params![turn.session.id, graph_id, config],
            )?;
        }
        transaction.execute(
            "UPDATE session_graph_turns SET status='completed'
             WHERE session_id=?1 AND turn_number=?2 AND status='running'",
            params![turn.session.id, turn.turn_number],
        )?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use beale_storage::{
        CommentaryRow, ModelUsageRow, ReasoningSummaryRow, ToolCallRow, TurnActivity, TurnMetadata,
        TurnRow,
    };

    fn root() -> std::path::PathBuf {
        static NEXT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        std::env::temp_dir().join(format!(
            "beale-feature-store-{}-{nanos}-{}",
            std::process::id(),
            NEXT.fetch_add(1, std::sync::atomic::Ordering::SeqCst)
        ))
    }

    fn open(root: &std::path::Path) -> HostDatabase {
        let db = HostDatabase::open(root).unwrap();
        migrate(&db).unwrap();
        db
    }

    #[test]
    fn peer_graph_reservations_and_handoffs_are_durable_and_isolated() {
        let root = root();
        let db = open(&root);
        let session = SessionRow {
            id: "session-example".to_owned(),
            workspace_id: "workspace-example".to_owned(),
            title: "Example research".to_owned(),
            created_at_ms: 100,
            last_message_at_ms: 100,
        };
        db.create_peer_graph_session(&session, "openai_pipeline", b"example-config", None)
            .unwrap();
        assert!(db.is_peer_graph_session(&session.id).unwrap());
        assert_eq!(
            db.sessions_excluding("internal:quick-chats")
                .unwrap()
                .as_slice(),
            std::slice::from_ref(&session)
        );
        let first = db
            .reserve_graph_turn("session-example", "discovery")
            .unwrap();
        let second = db
            .reserve_graph_turn("session-example", "validation")
            .unwrap();
        assert_ne!(first, second);
        assert_eq!(
            db.graph_turn_role("session-example", first)
                .unwrap()
                .as_deref(),
            Some("discovery")
        );
        assert_eq!(
            db.graph_turn_role("session-example", second)
                .unwrap()
                .as_deref(),
            Some("validation")
        );
        assert_eq!(
            db.graph_role_status("session-example", "discovery")
                .unwrap(),
            "working"
        );
        for (number, role) in [(first, "discovery"), (second, "validation")] {
            let provider_context = format!("[{{\"role\":\"{role}\"}}]");
            let tool = ToolCallRow {
                id: format!("tool-{role}-example"),
                turn_number: number,
                ordinal: 0,
                name: "read".to_owned(),
                summary: "Read example file".to_owned(),
                arguments: "{}".to_owned(),
                output: "Example output".to_owned(),
                is_error: false,
                agent_role: role.to_owned(),
                created_at_ms: 0,
            };
            let reasoning = ReasoningSummaryRow {
                turn_number: number,
                ordinal: 0,
                after_tool_count: 1,
                text: "Example reasoning".to_owned(),
                agent_role: role.to_owned(),
            };
            let commentary = CommentaryRow {
                turn_number: number,
                ordinal: 0,
                after_tool_count: 1,
                text: "Example progress".to_owned(),
                agent_role: role.to_owned(),
            };
            db.append_turn_with_metadata(
                &session,
                &TurnRow {
                    number,
                    provider: 1,
                    method: 1,
                    model: "example-model".to_owned(),
                    prompt: format!("{role} example task"),
                    answer: format!("{role} example answer"),
                    duration_ms: 1,
                    internal_prompt: false,
                },
                false,
                TurnActivity {
                    tools: &[tool],
                    reasoning: &[reasoning],
                    commentary: &[commentary],
                },
                None,
                TurnMetadata {
                    provider_context_json: Some(&provider_context),
                    agent_role: Some(role),
                    extensions: &[&GraphCommit { selection: None }],
                    ..TurnMetadata::default()
                },
            )
            .unwrap();
        }
        let discovery = db
            .graph_agent_turns("session-example", "discovery")
            .unwrap();
        let validation = db
            .graph_agent_turns("session-example", "validation")
            .unwrap();
        assert_eq!(discovery.len(), 1);
        assert_eq!(validation.len(), 1);
        assert_eq!(discovery[0].number, first);
        assert_eq!(validation[0].number, second);
        assert_eq!(
            db.graph_provider_context("session-example", "discovery")
                .unwrap(),
            Some("[{\"role\":\"discovery\"}]".to_owned())
        );
        assert_eq!(
            db.graph_provider_context("session-example", "validation")
                .unwrap(),
            Some("[{\"role\":\"validation\"}]".to_owned())
        );
        assert_eq!(db.provider_context("session-example").unwrap(), None);
        assert_eq!(
            db.tool_summaries_for_turns("session-example", &[second])
                .unwrap()[0]
                .agent_role,
            "validation"
        );
        assert_eq!(
            db.reasoning_summaries_for_turns("session-example", &[second])
                .unwrap()[0]
                .agent_role,
            "validation"
        );
        assert_eq!(
            db.commentary_for_turns("session-example", &[second])
                .unwrap()[0]
                .agent_role,
            "validation"
        );
        for index in 0..3 {
            let number = db
                .reserve_graph_turn("session-example", "discovery")
                .unwrap();
            db.append_turn_with_metadata(
                &session,
                &TurnRow {
                    number,
                    provider: 1,
                    method: 1,
                    model: "example-model".to_owned(),
                    prompt: format!("Discovery example followup {index}"),
                    answer: "Example answer".to_owned(),
                    duration_ms: 1,
                    internal_prompt: false,
                },
                false,
                TurnActivity {
                    tools: &[],
                    reasoning: &[],
                    commentary: &[],
                },
                None,
                TurnMetadata {
                    agent_role: Some("discovery"),
                    extensions: &[&GraphCommit { selection: None }],
                    ..TurnMetadata::default()
                },
            )
            .unwrap();
        }
        let recent = db.graph_recent_turns("session-example").unwrap();
        assert_eq!(recent.len(), 3);
        assert!(recent.iter().any(|turn| turn.number == second));
        assert!(!recent.iter().any(|turn| turn.number == first));
        assert_eq!(
            db.graph_provider_context("session-example", "discovery")
                .unwrap(),
            None
        );
        assert!(
            db.graph_provider_context("session-example", "validation")
                .unwrap()
                .is_some()
        );
        let handoff = GraphHandoffRow {
            id: "handoff-example".to_owned(),
            from_role: "discovery".to_owned(),
            to_role: "validation".to_owned(),
            task: "Check the example finding.".to_owned(),
            status: "pending".to_owned(),
        };
        db.enqueue_graph_handoff("session-example", &handoff, 101)
            .unwrap();
        assert_eq!(
            db.pending_graph_handoffs("session-example").unwrap(),
            [handoff]
        );
        db.set_graph_handoff_status("handoff-example", "running")
            .unwrap();
        db.reserve_graph_turn("session-example", "discovery")
            .unwrap();
        db.recover_abandoned_graph_work().unwrap();
        assert_eq!(
            db.graph_role_status("session-example", "discovery")
                .unwrap(),
            "failed"
        );
        assert_eq!(
            db.graph_role_status("session-example", "validation")
                .unwrap(),
            "queued"
        );
        assert_eq!(
            db.pending_graph_handoffs("session-example").unwrap().len(),
            1
        );
        drop(db);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn graph_agent_metrics_include_own_subagents_and_exclude_peers() {
        let root = root();
        let db = open(&root);
        let session = SessionRow {
            id: "session-example".to_owned(),
            workspace_id: "workspace-example".to_owned(),
            title: "Example research".to_owned(),
            created_at_ms: 1,
            last_message_at_ms: 2,
        };
        db.create_peer_graph_session(&session, "openai_pipeline", b"example-config", None)
            .unwrap();
        for (role, calls) in [("discovery", 2), ("validation", 1)] {
            let number = db.reserve_graph_turn(&session.id, role).unwrap();
            let usage = (0..calls)
                .map(|index| ModelUsageRow {
                    agent_id: if index == 0 { role } else { "scout-example" }.to_owned(),
                    role: if index == 0 { role } else { "scout" }.to_owned(),
                    provider: 1,
                    method: 1,
                    model: "example-model".to_owned(),
                    input_tokens: Some(100),
                    output_tokens: Some(10),
                    cache_read_tokens: None,
                    duration_ms: 100,
                    observed_at_ms: 1_000 + index,
                    work_elapsed_ms: Some(100 + index),
                })
                .collect::<Vec<_>>();
            db.append_turn_with_metadata(
                &session,
                &TurnRow {
                    number,
                    provider: 1,
                    method: 1,
                    model: "example-model".to_owned(),
                    prompt: "Example task".to_owned(),
                    answer: "Example answer".to_owned(),
                    duration_ms: 1_000,
                    internal_prompt: false,
                },
                false,
                TurnActivity {
                    tools: &[],
                    reasoning: &[],
                    commentary: &[],
                },
                None,
                TurnMetadata {
                    model_usage: Some(&usage),
                    agent_role: Some(role),
                    extensions: &[&GraphCommit { selection: None }],
                    ..Default::default()
                },
            )
            .unwrap();
        }
        let discovery = db
            .session_usage_totals_for_role(&session.id, Some("discovery"))
            .unwrap();
        let validation = db
            .session_usage_totals_for_role(&session.id, Some("validation"))
            .unwrap();
        assert_eq!(discovery.total_calls, 2);
        assert_eq!(validation.total_calls, 1);
        assert_eq!(discovery.input_tokens, 200);
        assert_eq!(
            db.session_model_totals_for_role(&session.id, Some("discovery"))
                .unwrap()
                .len(),
            2
        );
        let points = db
            .session_usage_snapshots_for_role(&session.id, Some("discovery"))
            .unwrap();
        assert_eq!(points.last().unwrap().totals.total_calls, 2);
        assert_eq!(points.last().unwrap().subagent_totals.total_calls, 1);
        assert_eq!(
            db.session_agent_duration_ms(&session.id, "discovery")
                .unwrap(),
            1_000
        );
        drop(db);
        std::fs::remove_dir_all(root).unwrap();
    }
}
