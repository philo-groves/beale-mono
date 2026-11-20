//! Memories persistence in the shared host database.
//!
//! This feature owns these tables and applies [`MIGRATIONS`] under its feature
//! ID; builds without the feature never create them.

use anyhow::{Result, ensure};
use beale_storage::{
    CommittedTurn, HostDatabase, TurnExtension,
    rusqlite::{self, OptionalExtension, Transaction, params},
};

/// Ordered schema versions for this feature's tables.
pub const MIGRATIONS: &[&str] = &[V1];

const V1: &str = "
CREATE TABLE memory_turn_outbox (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    workspace_id TEXT NOT NULL,
    session_id TEXT NOT NULL,
    turn_number INTEGER NOT NULL,
    agent_id TEXT NOT NULL,
    attempts INTEGER NOT NULL DEFAULT 0,
    next_attempt_at_ms INTEGER NOT NULL DEFAULT 0,
    status TEXT NOT NULL DEFAULT 'pending',
    UNIQUE(session_id, turn_number, agent_id),
    FOREIGN KEY(session_id, turn_number) REFERENCES session_turns(session_id, number) ON DELETE CASCADE
);
CREATE INDEX memory_turn_outbox_pending ON memory_turn_outbox(status, id);
";

/// Creates or upgrades this feature's tables.
pub fn migrate(db: &HostDatabase) -> Result<()> {
    db.migrate_schema(crate::FEATURE_ID, MIGRATIONS)
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MemoryTurnJob {
    pub id: i64,
    pub workspace_id: String,
    pub session_id: String,
    pub turn_number: i64,
    pub agent_id: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TranscriptItem {
    pub reference: String,
    pub turn_number: i64,
    pub actor_id: String,
    pub kind: String,
    pub text: String,
}

/// Memory extraction jobs and transcript reads on the host database.
pub trait MemoryJobStore {
    fn next_memory_job(&self, now_ms: i64) -> Result<Option<MemoryTurnJob>>;
    fn finish_memory_job(&self, id: i64) -> Result<()>;
    fn fail_memory_job(&self, id: i64, now_ms: i64) -> Result<()>;
    fn memory_job_counts(&self, workspace_id: &str) -> Result<(u64, u64)>;
    fn failed_memory_job_ids(&self, workspace_id: &str) -> Result<Vec<i64>>;
    fn clear_memory_jobs(&self, workspace_id: &str) -> Result<()>;
    fn retry_memory_job(&self, workspace_id: &str, id: i64) -> Result<bool>;
    fn memory_job_source(&self, job: &MemoryTurnJob) -> Result<String>;
    fn session_query(
        &self,
        workspace_id: &str,
        session_id: &str,
        text: &str,
        kind: &str,
        offset: usize,
        limit: usize,
    ) -> Result<Vec<TranscriptItem>>;
}

impl MemoryJobStore for HostDatabase {
    fn next_memory_job(&self, now_ms: i64) -> Result<Option<MemoryTurnJob>> {
        self.with_connection(|connection| {
            connection
                .query_row(
                    "SELECT id,workspace_id,session_id,turn_number,agent_id,next_attempt_at_ms
                 FROM memory_turn_outbox AS job WHERE status='pending'
                   AND EXISTS(SELECT 1 FROM feature_states WHERE id='beale-memories' AND enabled=1)
                   AND (COALESCE((SELECT all_workspaces FROM extension_scope_modes
                       WHERE kind='feature' AND extension_id='beale-memories'),1)=1
                     OR EXISTS(SELECT 1 FROM extension_workspace_scope
                       WHERE kind='feature' AND extension_id='beale-memories'
                         AND workspace_id=job.workspace_id))
                 ORDER BY id LIMIT 1",
                    [],
                    |row| {
                        let next: i64 = row.get(5)?;
                        Ok((
                            MemoryTurnJob {
                                id: row.get(0)?,
                                workspace_id: row.get(1)?,
                                session_id: row.get(2)?,
                                turn_number: row.get(3)?,
                                agent_id: row.get(4)?,
                            },
                            next,
                        ))
                    },
                )
                .optional()
                .map(|item| item.and_then(|(job, next)| (next <= now_ms).then_some(job)))
                .map_err(Into::into)
        })
    }

    fn finish_memory_job(&self, id: i64) -> Result<()> {
        self.with_connection(|connection| {
            connection.execute("DELETE FROM memory_turn_outbox WHERE id=?1", [id])?;
            Ok(())
        })
    }

    fn fail_memory_job(&self, id: i64, now_ms: i64) -> Result<()> {
        self.with_connection(|connection| {
            connection.execute(
                "UPDATE memory_turn_outbox SET attempts=attempts+1,
                 status=CASE WHEN attempts+1 >= 5 THEN 'failed' ELSE 'pending' END,
                 next_attempt_at_ms=?2 + MIN(3600000, 1000 * (1 << MIN(attempts, 12)))
                 WHERE id=?1",
                params![id, now_ms],
            )?;
            Ok(())
        })
    }

    fn memory_job_counts(&self, workspace_id: &str) -> Result<(u64, u64)> {
        self.with_connection(|connection| {
            let counts = connection.query_row(
                "SELECT COUNT(*) FILTER (WHERE status='pending'),
                        COUNT(*) FILTER (WHERE status='failed')
                 FROM memory_turn_outbox WHERE workspace_id=?1",
                [workspace_id],
                |row| Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)?)),
            )?;
            Ok((u64::try_from(counts.0)?, u64::try_from(counts.1)?))
        })
    }

    fn failed_memory_job_ids(&self, workspace_id: &str) -> Result<Vec<i64>> {
        self.with_connection(|connection| {
            let mut statement = connection.prepare(
                "SELECT id FROM memory_turn_outbox
                WHERE workspace_id=?1 AND status='failed' ORDER BY id LIMIT 50",
            )?;
            statement
                .query_map([workspace_id], |row| row.get(0))?
                .collect::<rusqlite::Result<Vec<_>>>()
                .map_err(Into::into)
        })
    }

    fn clear_memory_jobs(&self, workspace_id: &str) -> Result<()> {
        self.with_connection(|connection| {
            connection.execute(
                "DELETE FROM memory_turn_outbox WHERE workspace_id=?1",
                [workspace_id],
            )?;
            Ok(())
        })
    }

    fn retry_memory_job(&self, workspace_id: &str, id: i64) -> Result<bool> {
        self.with_connection(|connection| {
            Ok(connection.execute(
                "UPDATE memory_turn_outbox SET status='pending',attempts=0,
                next_attempt_at_ms=0 WHERE workspace_id=?1 AND id=?2 AND status='failed'",
                params![workspace_id, id],
            )? == 1)
        })
    }

    fn memory_job_source(&self, job: &MemoryTurnJob) -> Result<String> {
        self.with_connection(|connection| {
            let owner: String = connection.query_row(
                "SELECT workspace_id FROM sessions WHERE id=?1", [&job.session_id], |row| row.get(0)
            )?;
            ensure!(owner == job.workspace_id, "memory job workspace mismatch");
            if job.agent_id == "main" {
                let (prompt, answer, internal_prompt): (String, String, bool) = connection.query_row(
                    "SELECT prompt,answer,internal_prompt FROM session_turns WHERE session_id=?1 AND number=?2",
                    params![job.session_id, job.turn_number], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?))
                )?;
                let mut tools = connection.prepare(
                    "SELECT c.id,c.name,c.summary,substr(c.output,1,2000),c.is_error FROM session_tool_calls c
                     LEFT JOIN session_child_tools child ON child.tool_call_id=c.id
                     WHERE c.session_id=?1 AND c.turn_number=?2 AND child.tool_call_id IS NULL
                     ORDER BY c.ordinal LIMIT 64"
                )?;
                let tools = tools.query_map(params![job.session_id,job.turn_number], |row| {
                    Ok(serde_json::json!({"ref":row.get::<_,String>(0)?,"name":row.get::<_,String>(1)?,
                        "summary":row.get::<_,String>(2)?,"output":row.get::<_,String>(3)?,
                        "is_error":row.get::<_,bool>(4)?}))
                })?.collect::<rusqlite::Result<Vec<_>>>()?;
                let mut reasoning = connection.prepare(
                    "SELECT text FROM session_reasoning_summaries WHERE session_id=?1 AND turn_number=?2 ORDER BY ordinal LIMIT 32"
                )?;
                let reasoning = reasoning.query_map(params![job.session_id,job.turn_number], |row| row.get::<_,String>(0))?
                    .collect::<rusqlite::Result<Vec<_>>>()?;
                Ok(serde_json::json!({"actor":"main","prompt":prompt,"internal_prompt":internal_prompt,"answer":answer,
                    "tools":tools,"reasoning_summaries":reasoning}).to_string())
            } else {
                let (role, task, result, messages, reasoning): (String,String,String,String,String) = connection.query_row(
                    "SELECT role,task,result,messages_json,reasoning_json FROM session_child_turns
                     WHERE session_id=?1 AND parent_turn=?2 AND agent_id=?3",
                    params![job.session_id,job.turn_number,job.agent_id],
                    |row| Ok((row.get(0)?,row.get(1)?,row.get(2)?,row.get(3)?,row.get(4)?))
                )?;
                let mut tools = connection.prepare(
                    "SELECT c.id,c.name,c.summary,substr(c.output,1,2000),c.is_error
                     FROM session_tool_calls c JOIN session_child_tools ct ON ct.tool_call_id=c.id
                     WHERE c.session_id=?1 AND c.turn_number=?2 AND ct.agent_id=?3 ORDER BY c.ordinal LIMIT 64"
                )?;
                let tools = tools.query_map(params![job.session_id,job.turn_number,job.agent_id], |row| {
                    Ok(serde_json::json!({"ref":row.get::<_,String>(0)?,"name":row.get::<_,String>(1)?,
                        "summary":row.get::<_,String>(2)?,"output":row.get::<_,String>(3)?,
                        "is_error":row.get::<_,bool>(4)?}))
                })?.collect::<rusqlite::Result<Vec<_>>>()?;
                Ok(serde_json::json!({"actor":job.agent_id,"role":role,"task":task,
                    "result":result,"messages":serde_json::from_str::<serde_json::Value>(&messages)?,
                    "reasoning_summaries":serde_json::from_str::<serde_json::Value>(&reasoning)?,
                    "tools":tools}).to_string())
            }
        })
    }

    fn session_query(
        &self,
        workspace_id: &str,
        session_id: &str,
        text: &str,
        kind: &str,
        offset: usize,
        limit: usize,
    ) -> Result<Vec<TranscriptItem>> {
        ensure!(
            limit <= 50 && offset <= 10_000 && text.len() <= 256,
            "invalid session query"
        );
        self.with_connection(|connection| {
            let mut statement = connection.prepare(
                "WITH items AS (
                   SELECT 'turn:'||t.number||':user' ref,t.number turn_number,COALESCE(t.agent_role,'main') actor_id,'message' kind,t.prompt body,0 ordinal
                     FROM session_turns t WHERE t.session_id=?1
                   UNION ALL SELECT 'turn:'||t.number||':assistant',t.number,COALESCE(t.agent_role,'main'),'message',t.answer,9000 FROM session_turns t WHERE t.session_id=?1
                   UNION ALL SELECT 'tool:'||c.id,c.turn_number,COALESCE(ct.agent_id,c.agent_role,'main'),'tool',c.summary||char(10)||substr(c.output,1,8000),c.ordinal*10+1 FROM session_tool_calls c LEFT JOIN session_child_tools ct ON ct.tool_call_id=c.id WHERE c.session_id=?1
                   UNION ALL SELECT 'reasoning:'||r.turn_number||':'||r.ordinal,r.turn_number,COALESCE(r.agent_role,'main'),'reasoning_summary',r.text,r.after_tool_count*10+2 FROM session_reasoning_summaries r WHERE r.session_id=?1
                   UNION ALL SELECT 'commentary:'||c.turn_number||':'||c.ordinal,c.turn_number,COALESCE(c.agent_role,'main'),'commentary',c.text,c.after_tool_count*10+3 FROM session_commentary c WHERE c.session_id=?1
                   UNION ALL SELECT 'child:'||c.agent_id||':task',c.parent_turn,c.agent_id,'message',c.task,4 FROM session_child_turns c WHERE c.session_id=?1
                   UNION ALL SELECT 'child:'||c.agent_id||':message:'||m.key,c.parent_turn,c.agent_id,'message',json_extract(m.value,'$.text'),5000+CAST(m.key AS INTEGER) FROM session_child_turns c,json_each(c.messages_json) m WHERE c.session_id=?1 AND json_extract(m.value,'$.role')='assistant'
                   UNION ALL SELECT 'child:'||c.agent_id||':reasoning:'||r.key,c.parent_turn,c.agent_id,'reasoning_summary',r.value,6000+CAST(r.key AS INTEGER) FROM session_child_turns c,json_each(c.reasoning_json) r WHERE c.session_id=?1
                   UNION ALL SELECT 'child:'||c.agent_id||':result',c.parent_turn,c.agent_id,'message',c.result,8000 FROM session_child_turns c WHERE c.session_id=?1
                 ) SELECT ref,turn_number,actor_id,kind,substr(body,1,8000) FROM items
                 WHERE (?3='' OR kind=?3) AND (?4='' OR instr(lower(body),lower(?4))>0)
                   AND EXISTS(SELECT 1 FROM sessions s WHERE s.id=?1 AND s.workspace_id=?2)
                   AND (ref LIKE 'turn:%' OR ref LIKE 'tool:%' OR ref LIKE 'reasoning:%' OR ref LIKE 'commentary:%' OR ref LIKE 'child:%')
                 ORDER BY turn_number DESC,ordinal DESC LIMIT ?5 OFFSET ?6"
            )?;
            let rows = statement.query_map(params![session_id,workspace_id,kind,text,limit as i64,offset as i64], |row| {
                Ok(TranscriptItem { reference: row.get(0)?, turn_number: row.get(1)?, actor_id: row.get(2)?,
                    kind: row.get(3)?, text: row.get(4)? })
            })?;
            rows.collect::<rusqlite::Result<Vec<_>>>().map_err(Into::into)
        })
    }
}

/// Queues memory extraction for the main agent and each subagent of a
/// committed turn.
pub struct QueueMemoryJobs;

impl TurnExtension for QueueMemoryJobs {
    fn commit(&self, transaction: &Transaction<'_>, turn: &CommittedTurn<'_>) -> Result<()> {
        let session = turn.session;
        // Subagent jobs precede the main agent's job, matching completion order.
        let main = turn.agent_role.unwrap_or("main");
        let children = turn.child_turns.iter().map(|child| child.agent_id.as_str());
        for agent_id in children.chain(std::iter::once(main)) {
            transaction.execute(
                "INSERT INTO memory_turn_outbox(workspace_id,session_id,turn_number,agent_id) VALUES (?1,?2,?3,?4)",
                params![session.workspace_id, session.id, turn.turn_number, agent_id],
            )?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use beale_storage::{
        ChildTurnRow, SessionRow, ToolCallRow, TurnActivity, TurnMetadata, TurnRow,
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
    fn memory_outbox_and_child_transcript_commit_with_turn_and_remain_workspace_scoped() {
        let root = root();
        let db = open(&root);
        for (session_id, workspace_id, prompt) in [
            (
                "session-example-a",
                "workspace-example-a",
                "Example alpha prompt",
            ),
            (
                "session-example-b",
                "workspace-example-b",
                "Example beta prompt",
            ),
        ] {
            let session = SessionRow {
                id: session_id.to_owned(),
                workspace_id: workspace_id.to_owned(),
                title: "Example session".to_owned(),
                created_at_ms: 1,
                last_message_at_ms: 2,
            };
            let turn = TurnRow {
                number: 1,
                provider: 1,
                method: 1,
                model: "example-model".to_owned(),
                prompt: prompt.to_owned(),
                answer: "Example answer".to_owned(),
                duration_ms: 1,
                internal_prompt: false,
            };
            let tool_id = format!("tool-{session_id}");
            let tool = ToolCallRow {
                id: tool_id.clone(),
                turn_number: 1,
                ordinal: 1,
                name: "read".to_owned(),
                summary: "Inspect synthetic fixture".to_owned(),
                arguments: "{}".to_owned(),
                output: "Example tool output".to_owned(),
                is_error: false,
                agent_role: "main".to_owned(),
                created_at_ms: 0,
            };
            let child = ChildTurnRow {
                agent_id: format!("child-{session_id}"),
                role: "scout".to_owned(),
                task: "Inspect example".to_owned(),
                result: "Observed synthetic result".to_owned(),
                is_error: false,
                messages_json: serde_json::json!([
                    {"role":"assistant","text":"Example child message"},
                    {"role":"tool","tool_call_id":tool_id,"name":"read"}
                ])
                .to_string(),
                reasoning_json: "[\"Example reasoning summary\"]".to_owned(),
                completed_at_ms: 2,
            };
            db.append_turn_with_metadata(
                &session,
                &turn,
                true,
                TurnActivity {
                    tools: &[tool],
                    reasoning: &[],
                    commentary: &[],
                },
                None,
                TurnMetadata {
                    child_turns: &[child],
                    extensions: &[&QueueMemoryJobs],
                    ..TurnMetadata::default()
                },
            )
            .unwrap();
        }
        db.set_feature_enabled("beale-memories", true).unwrap();
        let first = db.next_memory_job(2).unwrap().unwrap();
        assert_eq!(first.session_id, "session-example-a");
        assert!(first.agent_id.starts_with("child-"));
        assert!(
            db.memory_job_source(&first)
                .unwrap()
                .contains("Observed synthetic result")
        );
        let items = db
            .session_query(
                "workspace-example-a",
                "session-example-a",
                "alpha",
                "",
                0,
                20,
            )
            .unwrap();
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].text, "Example alpha prompt");
        let tool_items = db
            .session_query(
                "workspace-example-a",
                "session-example-a",
                "Example tool",
                "tool",
                0,
                20,
            )
            .unwrap();
        assert_eq!(tool_items.len(), 1);
        assert_eq!(tool_items[0].actor_id, "child-session-example-a");
        let reasoning_items = db
            .session_query(
                "workspace-example-a",
                "session-example-a",
                "reasoning",
                "reasoning_summary",
                0,
                20,
            )
            .unwrap();
        assert_eq!(reasoning_items.len(), 1);
        assert_eq!(reasoning_items[0].actor_id, "child-session-example-a");
        assert!(
            db.session_query(
                "workspace-example-b",
                "session-example-a",
                "alpha",
                "",
                0,
                20
            )
            .unwrap()
            .is_empty()
        );
        for _ in 0..5 {
            db.fail_memory_job(first.id, 2).unwrap();
        }
        assert_eq!(db.memory_job_counts("workspace-example-a").unwrap().1, 1);
        assert!(
            !db.retry_memory_job("workspace-example-b", first.id)
                .unwrap()
        );
        assert!(
            db.retry_memory_job("workspace-example-a", first.id)
                .unwrap()
        );
        assert_eq!(db.next_memory_job(2).unwrap().unwrap().id, first.id);
        db.finish_memory_job(first.id).unwrap();
        assert_eq!(db.next_memory_job(2).unwrap().unwrap().agent_id, "main");
        drop(db);
        std::fs::remove_dir_all(root).unwrap();
    }
}
