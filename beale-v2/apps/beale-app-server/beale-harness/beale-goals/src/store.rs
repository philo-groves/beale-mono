//! Goals persistence in the shared host database.
//!
//! Goal Mode owns these tables and applies [`MIGRATIONS`] under its schema ID.

use anyhow::{Result, ensure};
use beale_storage::{
    CommittedTurn, HostDatabase, TurnExtension,
    rusqlite::{OptionalExtension, Transaction, params},
};

/// Ordered schema versions for Goal Mode tables.
pub const MIGRATIONS: &[&str] = &[V1];

const V1: &str = "
CREATE TABLE session_goals (
    session_id TEXT PRIMARY KEY REFERENCES sessions(id) ON DELETE CASCADE,
    objective TEXT NOT NULL,
    status INTEGER NOT NULL,
    turns_used INTEGER NOT NULL,
    required_state TEXT NOT NULL,
    max_turns INTEGER NOT NULL DEFAULT 0,
    max_duration_minutes INTEGER NOT NULL DEFAULT 0,
    active_ms INTEGER NOT NULL DEFAULT 0
);
";

/// Creates or upgrades Goal Mode tables.
pub fn migrate(db: &HostDatabase) -> Result<()> {
    db.migrate_schema(crate::FEATURE_ID, MIGRATIONS)
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GoalRow {
    pub session_id: String,
    pub objective: String,
    pub status: i32,
    pub turns_used: i64,
    pub required_state: String,
    pub max_turns: i64,
    pub max_duration_minutes: i64,
    pub active_ms: i64,
}

/// Goal Mode state on the host database.
pub trait GoalStore {
    fn goal(&self, session_id: &str) -> Result<Option<GoalRow>>;
    fn set_goal_status(&self, session_id: &str, status: i32) -> Result<()>;
}

impl GoalStore for HostDatabase {
    fn goal(&self, session_id: &str) -> Result<Option<GoalRow>> {
        self.with_connection(|connection| {
            connection.query_row(
                "SELECT session_id, objective, status, turns_used, required_state, max_turns, max_duration_minutes, active_ms FROM session_goals WHERE session_id = ?1",
                [session_id],
                |row| Ok(GoalRow {
                    session_id: row.get(0)?,
                    objective: row.get(1)?,
                    status: row.get(2)?,
                    turns_used: row.get(3)?,
                    required_state: row.get(4)?,
                    max_turns: row.get(5)?,
                    max_duration_minutes: row.get(6)?,
                    active_ms: row.get(7)?,
                }),
            ).optional().map_err(Into::into)
        })
    }

    fn set_goal_status(&self, session_id: &str, status: i32) -> Result<()> {
        self.with_connection(|connection| {
            connection.execute(
                "UPDATE session_goals SET status = ?1 WHERE session_id = ?2",
                params![status, session_id],
            )?;
            Ok(())
        })
    }
}

/// Saves Goal Mode progress with the turn that produced it.
pub struct GoalCommit<'a>(pub &'a GoalRow);

impl TurnExtension for GoalCommit<'_> {
    fn commit(&self, transaction: &Transaction<'_>, turn: &CommittedTurn<'_>) -> Result<()> {
        let goal = self.0;
        ensure!(goal.session_id == turn.session.id, "goal session mismatch");
        transaction.execute(
            "INSERT INTO session_goals(session_id, objective, status, turns_used, required_state, max_turns, max_duration_minutes, active_ms)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
             ON CONFLICT(session_id) DO UPDATE SET status = excluded.status,
                 turns_used = excluded.turns_used, required_state = excluded.required_state,
                 active_ms = excluded.active_ms",
            params![goal.session_id, goal.objective, goal.status, goal.turns_used, goal.required_state, goal.max_turns, goal.max_duration_minutes, goal.active_ms],
        )?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use beale_storage::{SessionRow, TurnActivity, TurnMetadata, TurnRow, WorkspaceRow};

    fn root() -> std::path::PathBuf {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        std::env::temp_dir().join(format!("beale-goals-{}-{nanos}", std::process::id()))
    }

    fn goal(session_id: &str) -> GoalRow {
        GoalRow {
            session_id: session_id.to_owned(),
            objective: "Example objective".to_owned(),
            status: 1,
            turns_used: 1,
            required_state: String::new(),
            max_turns: 0,
            max_duration_minutes: 0,
            active_ms: 1,
        }
    }

    fn commit(db: &HostDatabase, workspace_id: &str, goal: &GoalRow) -> Result<()> {
        db.append_turn_with_metadata(
            &SessionRow {
                id: format!("session-{workspace_id}"),
                workspace_id: workspace_id.to_owned(),
                title: "Example objective".to_owned(),
                created_at_ms: 1,
                last_message_at_ms: 1,
            },
            &TurnRow {
                number: 1,
                provider: 1,
                method: 1,
                model: "example-model".to_owned(),
                prompt: "Example objective".to_owned(),
                answer: "Example observation".to_owned(),
                duration_ms: 1,
                internal_prompt: false,
            },
            true,
            TurnActivity {
                tools: &[],
                reasoning: &[],
                commentary: &[],
            },
            None,
            TurnMetadata {
                extensions: &[&GoalCommit(goal)],
                ..TurnMetadata::default()
            },
        )
    }

    #[test]
    fn goals_commit_with_turns_and_update_status() {
        let root = root();
        let db = HostDatabase::open(&root).unwrap();
        migrate(&db).unwrap();
        db.replace_workspaces(&[
            WorkspaceRow {
                id: "workspace-a".to_owned(),
                name: "Example A".to_owned(),
                directory: "example-a".to_owned(),
            },
            WorkspaceRow {
                id: "workspace-b".to_owned(),
                name: "Example B".to_owned(),
                directory: "example-b".to_owned(),
            },
        ])
        .unwrap();
        assert!(commit(&db, "workspace-a", &goal("session-other")).is_err());
        assert!(db.session("session-workspace-a").unwrap().is_none());
        for workspace_id in ["workspace-a", "workspace-b"] {
            commit(&db, workspace_id, &goal(&format!("session-{workspace_id}"))).unwrap();
        }
        db.set_goal_status("session-workspace-a", 4).unwrap();
        assert_eq!(db.goal("session-workspace-a").unwrap().unwrap().status, 4);
        assert_eq!(db.goal("session-workspace-b").unwrap().unwrap().status, 1);
        drop(db);
        std::fs::remove_dir_all(root).unwrap();
    }
}
