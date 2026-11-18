use anyhow::{Context, Result, bail, ensure};
use chrono::{DateTime, Datelike, Months, Utc};
use rusqlite::{Connection, OptionalExtension, params};
use std::{
    collections::{BTreeMap, HashSet},
    path::Path,
    sync::{Arc, Mutex},
    time::Duration,
};

mod findings;
pub use findings::now_ms as finding_now_ms;
mod runbooks;
pub use runbooks::RunbookRunRow;

#[derive(Clone)]
pub struct HostDatabase(Arc<Mutex<Connection>>);

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ManualBrowserRow {
    pub id: String,
    pub name: String,
    pub executable: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BrowserSettings {
    pub enabled: bool,
    pub all_workspaces: bool,
    pub workspace_ids: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VirtualMachineSettings {
    pub enabled: bool,
    pub all_workspaces: bool,
    pub workspace_ids: Vec<String>,
    pub can_run: bool,
    pub can_clone: bool,
}

impl Default for VirtualMachineSettings {
    fn default() -> Self {
        Self {
            enabled: true,
            all_workspaces: true,
            workspace_ids: Vec::new(),
            can_run: false,
            can_clone: false,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WorkspaceRow {
    pub id: String,
    pub name: String,
    pub directory: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExtensionWorkspaceScope {
    pub all_workspaces: bool,
    pub workspace_ids: Vec<String>,
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
pub struct AutomationJobRow {
    pub id: String,
    pub session_id: String,
    pub workspace_id: String,
    pub title: String,
    pub repeat_preset: String,
    pub next_run_at_ms: i64,
    pub last_run_at_ms: i64,
    pub canceled: bool,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct AutomationOptions {
    pub reasoning_effort: i32,
    pub goal_mode: bool,
    pub goal_max_turns: u64,
    pub goal_max_duration_minutes: u64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AutomationRunRow {
    pub id: String,
    pub job_id: String,
    pub session_id: Option<String>,
    pub title: String,
    pub started_at_ms: i64,
    pub completed_at_ms: Option<i64>,
    pub status: i32,
    pub error: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AutomationClaim {
    pub run_id: String,
    pub job_id: String,
    pub workspace_id: String,
    pub provider: i32,
    pub method: i32,
    pub model: String,
    pub prompt: String,
    pub options: AutomationOptions,
}

pub fn automation_next_run(preset: &str, anchor_ms: i64, occurrence: u32) -> Result<i64> {
    ensure!(occurrence > 0, "invalid automation occurrence");
    let interval: i64 = match preset {
        "minute" => 60_000,
        "hour" => 3_600_000,
        "day" => 86_400_000,
        "week" => 604_800_000,
        "month" => {
            let date = DateTime::<Utc>::from_timestamp_millis(anchor_ms)
                .context("invalid automation time")?;
            return date
                .checked_add_months(Months::new(occurrence))
                .map(|next| next.timestamp_millis())
                .context("automation time overflow");
        }
        _ => bail!("invalid automation repeat preset"),
    };
    anchor_ms
        .checked_add(
            interval
                .checked_mul(i64::from(occurrence))
                .context("automation time overflow")?,
        )
        .context("automation time overflow")
}

fn next_automation_after(
    preset: &str,
    anchor_ms: i64,
    minimum: u32,
    now_ms: i64,
) -> Result<(u32, i64)> {
    let mut occurrence = minimum.max(1);
    if preset == "month" {
        let anchor = DateTime::<Utc>::from_timestamp_millis(anchor_ms)
            .context("invalid automation anchor")?;
        let now = DateTime::<Utc>::from_timestamp_millis(now_ms).context("invalid current time")?;
        let months = (now.year() - anchor.year()) * 12 + now.month() as i32 - anchor.month() as i32;
        occurrence = occurrence.max(u32::try_from(months.max(0))?);
    } else {
        let interval = automation_next_run(preset, 0, 1)?;
        if now_ms >= anchor_ms {
            let elapsed = now_ms
                .checked_sub(anchor_ms)
                .context("automation time overflow")?;
            occurrence = occurrence.max(u32::try_from(elapsed / interval + 1)?);
        }
    }
    loop {
        let due = automation_next_run(preset, anchor_ms, occurrence)?;
        if due > now_ms {
            return Ok((occurrence, due));
        }
        occurrence = occurrence
            .checked_add(1)
            .context("automation occurrence overflow")?;
    }
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
    pub internal_prompt: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ChildTurnRow {
    pub agent_id: String,
    pub role: String,
    pub task: String,
    pub result: String,
    pub is_error: bool,
    pub messages_json: String,
    pub reasoning_json: String,
    pub completed_at_ms: i64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SessionChildRow {
    pub parent_role: String,
    pub turn: ChildTurnRow,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SessionChildSummaryRow {
    pub parent_role: String,
    pub agent_id: String,
    pub role: String,
    pub task: String,
    pub is_error: bool,
    pub completed_at_ms: i64,
}

impl From<&SessionChildRow> for SessionChildSummaryRow {
    fn from(row: &SessionChildRow) -> Self {
        Self {
            parent_role: row.parent_role.clone(),
            agent_id: row.turn.agent_id.clone(),
            role: row.turn.role.clone(),
            task: row.turn.task.clone(),
            is_error: row.turn.is_error,
            completed_at_ms: row.turn.completed_at_ms,
        }
    }
}

fn session_child_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<SessionChildRow> {
    Ok(SessionChildRow {
        parent_role: row.get(0)?,
        turn: ChildTurnRow {
            agent_id: row.get(1)?,
            role: row.get(2)?,
            task: row.get(3)?,
            result: row.get(4)?,
            is_error: row.get(5)?,
            messages_json: row.get(6)?,
            reasoning_json: row.get(7)?,
            completed_at_ms: row.get(8)?,
        },
    })
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ChildAssistantMessageRow {
    pub parent_turn: i64,
    pub role: String,
    pub text: String,
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

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SubscriptionUsageRow {
    pub provider: i32,
    pub window_id: String,
    pub label: String,
    pub observed_at_ms: i64,
    pub used_percent: i64,
    pub resets_at_unix: i64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ModelUsageRow {
    pub agent_id: String,
    pub role: String,
    pub provider: i32,
    pub method: i32,
    pub model: String,
    pub input_tokens: Option<i64>,
    pub output_tokens: Option<i64>,
    pub cache_read_tokens: Option<i64>,
    pub work_elapsed_ms: Option<i64>,
    pub duration_ms: i64,
    pub observed_at_ms: i64,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SessionUsageTotals {
    pub input_tokens: i64,
    pub output_tokens: i64,
    pub measured_calls: i64,
    pub total_calls: i64,
    pub cache_read_tokens: i64,
    pub cache_input_tokens: i64,
    pub cache_measured_calls: i64,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SessionUsageSnapshotRow {
    pub work_duration_ms: u64,
    pub totals: SessionUsageTotals,
    pub main_totals: SessionUsageTotals,
    pub subagent_totals: SessionUsageTotals,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SessionModelUsageRow {
    pub provider: i32,
    pub method: i32,
    pub model: String,
    pub role: String,
    pub totals: SessionUsageTotals,
}

fn is_top_level_role(role: &str) -> bool {
    matches!(
        role,
        "main"
            | "discovery_harness"
            | "validation_system"
            | "discovery"
            | "validation"
            | "reporter"
            | "patch_generation"
    )
}

fn add_usage_totals(total: &mut SessionUsageTotals, delta: &SessionUsageTotals) {
    total.input_tokens = total.input_tokens.saturating_add(delta.input_tokens);
    total.output_tokens = total.output_tokens.saturating_add(delta.output_tokens);
    total.measured_calls = total.measured_calls.saturating_add(delta.measured_calls);
    total.total_calls = total.total_calls.saturating_add(delta.total_calls);
    total.cache_read_tokens = total
        .cache_read_tokens
        .saturating_add(delta.cache_read_tokens);
    total.cache_input_tokens = total
        .cache_input_tokens
        .saturating_add(delta.cache_input_tokens);
    total.cache_measured_calls = total
        .cache_measured_calls
        .saturating_add(delta.cache_measured_calls);
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ModelUsageDayRow {
    pub day_start_ms: i64,
    pub role: String,
    pub provider: i32,
    pub method: i32,
    pub model: String,
    pub input_tokens: i64,
    pub output_tokens: i64,
    pub duration_ms: i64,
    pub measured_calls: i64,
    pub total_calls: i64,
    pub cache_read_tokens: i64,
    pub cache_input_tokens: i64,
    pub cache_measured_calls: i64,
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
    pub agent_role: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ToolCallSummaryRow {
    pub id: String,
    pub turn_number: i64,
    pub ordinal: i64,
    pub name: String,
    pub summary: String,
    pub is_error: bool,
    pub agent_role: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReasoningSummaryRow {
    pub turn_number: i64,
    pub ordinal: i64,
    pub after_tool_count: i64,
    pub text: String,
    pub agent_role: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CommentaryRow {
    pub turn_number: i64,
    pub ordinal: i64,
    pub after_tool_count: i64,
    pub text: String,
    pub agent_role: String,
}

pub struct TurnActivity<'a> {
    pub tools: &'a [ToolCallRow],
    pub reasoning: &'a [ReasoningSummaryRow],
    pub commentary: &'a [CommentaryRow],
    pub goal: Option<&'a GoalRow>,
}

#[derive(Default)]
pub struct TurnMetadata<'a> {
    pub provider_context_json: Option<&'a str>,
    pub graph_id: Option<&'a str>,
    pub graph_config: Option<&'a [u8]>,
    pub model_usage: Option<&'a [ModelUsageRow]>,
    pub child_turns: &'a [ChildTurnRow],
    pub queue_memories: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GraphHandoffRow {
    pub id: String,
    pub from_role: String,
    pub to_role: String,
    pub task: String,
    pub status: String,
}

#[derive(Clone, Debug)]
pub struct SessionWithTurns {
    pub session: SessionRow,
    pub turns: Vec<TurnRow>,
}

impl HostDatabase {
    pub fn protect_database_file(root: &Path, path: &Path) -> Result<()> {
        private_root(root)?;
        ensure!(
            path.parent() == Some(root),
            "database path must be beneath app-server root"
        );
        #[cfg(windows)]
        restrict_windows_acl(path, false)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))
                .context("restrict database file")?;
        }
        Ok(())
    }

    pub fn open(root: &Path) -> Result<Self> {
        private_root(root)?;
        let path = root.join("state.sqlite");
        let mut connection = Connection::open(&path).context("open host database")?;
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
        if version > 47 {
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
        if version < 13 {
            connection.execute_batch(
                "BEGIN IMMEDIATE;
                 ALTER TABLE session_turns ADD COLUMN internal_prompt INTEGER NOT NULL DEFAULT 0;
                 CREATE TABLE session_goals (
                     session_id TEXT PRIMARY KEY REFERENCES sessions(id) ON DELETE CASCADE,
                     objective TEXT NOT NULL,
                     status INTEGER NOT NULL,
                     turns_used INTEGER NOT NULL,
                     required_state TEXT NOT NULL
                 );
                 PRAGMA user_version = 13;
                 COMMIT;",
            )?;
        }
        if version < 14 {
            connection.execute_batch(
                "BEGIN IMMEDIATE;
                 ALTER TABLE session_goals ADD COLUMN max_turns INTEGER NOT NULL DEFAULT 0;
                 ALTER TABLE session_goals ADD COLUMN max_duration_minutes INTEGER NOT NULL DEFAULT 0;
                 ALTER TABLE session_goals ADD COLUMN active_ms INTEGER NOT NULL DEFAULT 0;
                 PRAGMA user_version = 14;
                 COMMIT;",
            )?;
        }
        if version < 15 {
            connection.execute_batch(
                "BEGIN IMMEDIATE;
                 CREATE TABLE IF NOT EXISTS feature_states (
                     id TEXT PRIMARY KEY,
                     enabled INTEGER NOT NULL CHECK (enabled IN (0, 1))
                 );
                 UPDATE session_goals SET status = 4 WHERE status = 1;
                 PRAGMA user_version = 15;
                 COMMIT;",
            )?;
        }
        if version < 16 {
            connection.execute_batch(
                "BEGIN IMMEDIATE;
                 CREATE TABLE IF NOT EXISTS extension_scope_modes (
                     kind TEXT NOT NULL CHECK (kind IN ('plugin', 'feature')),
                     extension_id TEXT NOT NULL,
                     all_workspaces INTEGER NOT NULL CHECK (all_workspaces IN (0, 1)),
                     PRIMARY KEY(kind, extension_id)
                 );
                 CREATE TABLE IF NOT EXISTS extension_workspace_scope (
                     kind TEXT NOT NULL CHECK (kind IN ('plugin', 'feature')),
                     extension_id TEXT NOT NULL,
                     workspace_id TEXT NOT NULL,
                     PRIMARY KEY(kind, extension_id, workspace_id)
                 );
                 CREATE INDEX IF NOT EXISTS extension_workspace_scope_workspace ON extension_workspace_scope(workspace_id);
                 PRAGMA user_version = 16;
                 COMMIT;",
            )?;
        }
        if version < 17 {
            connection.execute_batch(
                "BEGIN IMMEDIATE;
                 CREATE TABLE IF NOT EXISTS fleet_totp_secret (
                     id INTEGER PRIMARY KEY CHECK (id = 1),
                     secret BLOB NOT NULL CHECK (length(secret) = 20)
                 );
                 PRAGMA user_version = 17;
                 COMMIT;",
            )?;
        }
        if version < 18 {
            connection.execute_batch(
                "BEGIN IMMEDIATE;
                 CREATE TABLE IF NOT EXISTS manual_browsers (
                     id TEXT PRIMARY KEY,
                     name TEXT NOT NULL,
                     executable TEXT NOT NULL UNIQUE
                 );
                 PRAGMA user_version = 18;
                 COMMIT;",
            )?;
        }
        if version < 19 {
            connection.execute_batch(
                "BEGIN IMMEDIATE;
                 CREATE TABLE IF NOT EXISTS fleet_browser_settings (
                     browser_id TEXT PRIMARY KEY,
                     enabled INTEGER NOT NULL CHECK (enabled IN (0, 1)),
                     all_workspaces INTEGER NOT NULL CHECK (all_workspaces IN (0, 1))
                 );
                 CREATE TABLE IF NOT EXISTS fleet_browser_workspace_scope (
                     browser_id TEXT NOT NULL,
                     workspace_id TEXT NOT NULL,
                     PRIMARY KEY(browser_id, workspace_id),
                     FOREIGN KEY(browser_id) REFERENCES fleet_browser_settings(browser_id) ON DELETE CASCADE
                 );
                 PRAGMA user_version = 19;
                 COMMIT;",
            )?;
        }
        if version < 20 {
            connection.execute_batch(
                "BEGIN IMMEDIATE;
                 CREATE TABLE IF NOT EXISTS fleet_preferred_browser (
                     id INTEGER PRIMARY KEY CHECK (id = 1),
                     browser_id TEXT NOT NULL
                 );
                 PRAGMA user_version = 20;
                 COMMIT;",
            )?;
        }
        if version < 21 {
            connection.execute_batch(
                "BEGIN IMMEDIATE;
                 CREATE TABLE IF NOT EXISTS fleet_vm_settings (
                     vm_id TEXT PRIMARY KEY,
                     enabled INTEGER NOT NULL CHECK (enabled IN (0, 1)),
                     all_workspaces INTEGER NOT NULL CHECK (all_workspaces IN (0, 1)),
                     can_run INTEGER NOT NULL CHECK (can_run IN (0, 1)),
                     can_clone INTEGER NOT NULL CHECK (can_clone IN (0, 1))
                 );
                 CREATE TABLE IF NOT EXISTS fleet_vm_workspace_scope (
                     vm_id TEXT NOT NULL,
                     workspace_id TEXT NOT NULL,
                     PRIMARY KEY(vm_id, workspace_id),
                     FOREIGN KEY(vm_id) REFERENCES fleet_vm_settings(vm_id) ON DELETE CASCADE
                 );
                 PRAGMA user_version = 21;
                 COMMIT;",
            )?;
        }
        if version < 22 {
            connection.execute_batch(
                "BEGIN IMMEDIATE;
                 CREATE TABLE IF NOT EXISTS automation_jobs (
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
                 CREATE INDEX IF NOT EXISTS automation_jobs_next_run ON automation_jobs(canceled, next_run_at_ms);
                 PRAGMA user_version = 22;
                 COMMIT;",
            )?;
        }
        if version < 23 {
            connection.execute_batch(
                "BEGIN IMMEDIATE;
                 CREATE TABLE IF NOT EXISTS automation_job_options (
                     job_id TEXT PRIMARY KEY REFERENCES automation_jobs(id) ON DELETE CASCADE,
                     reasoning_effort INTEGER NOT NULL,
                     goal_mode INTEGER NOT NULL CHECK (goal_mode IN (0, 1)),
                     goal_max_turns INTEGER NOT NULL,
                     goal_max_duration_minutes INTEGER NOT NULL,
                     anchor_at_ms INTEGER NOT NULL
                 );
                 CREATE TABLE IF NOT EXISTS automation_runs (
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
                 CREATE INDEX IF NOT EXISTS automation_runs_job_status ON automation_runs(job_id, status);
                 INSERT OR IGNORE INTO automation_job_options
                     (job_id, reasoning_effort, goal_mode, goal_max_turns, goal_max_duration_minutes, anchor_at_ms)
                 SELECT id, 0, 0, 0, 0, last_run_at_ms FROM automation_jobs;
                 INSERT OR IGNORE INTO automation_runs
                     (id, job_id, session_id, title, started_at_ms, completed_at_ms, status)
                 SELECT a.id, a.id, a.session_id, a.title, s.created_at_ms,
                        a.last_run_at_ms, 2
                 FROM automation_jobs a JOIN sessions s ON s.id = a.session_id;
                 PRAGMA user_version = 23;
                 COMMIT;",
            )?;
        }
        if version < 24 {
            connection.execute_batch(
                "BEGIN IMMEDIATE;
                 CREATE TABLE IF NOT EXISTS session_provider_context (
                     session_id TEXT PRIMARY KEY REFERENCES sessions(id) ON DELETE CASCADE,
                     context_json TEXT NOT NULL
                 );
                 PRAGMA user_version = 24;
                 COMMIT;",
            )?;
        }
        if version < 25 {
            connection.execute_batch(
                "BEGIN IMMEDIATE;
                 CREATE TABLE IF NOT EXISTS session_graphs (
                     session_id TEXT PRIMARY KEY REFERENCES sessions(id) ON DELETE CASCADE,
                     graph_id TEXT NOT NULL
                 );
                 PRAGMA user_version = 25;
                 COMMIT;",
            )?;
        }
        if version < 26 {
            let has_config: bool = connection.query_row(
                "SELECT EXISTS(SELECT 1 FROM pragma_table_info('session_graphs') WHERE name = 'config_blob')",
                [],
                |row| row.get(0),
            )?;
            if has_config {
                connection.execute_batch("PRAGMA user_version = 26;")?;
            } else {
                connection.execute_batch(
                    "BEGIN IMMEDIATE;
                     ALTER TABLE session_graphs ADD COLUMN config_blob BLOB NOT NULL DEFAULT X'';
                     PRAGMA user_version = 26;
                     COMMIT;",
                )?;
            }
        }
        if version < 27 {
            connection.execute_batch(
                "BEGIN IMMEDIATE;
                 CREATE TABLE IF NOT EXISTS subscription_usage_history (
                     provider INTEGER NOT NULL, window_id TEXT NOT NULL, label TEXT NOT NULL,
                     observed_at_ms INTEGER NOT NULL, used_percent INTEGER NOT NULL,
                     resets_at_unix INTEGER NOT NULL,
                     PRIMARY KEY(provider, window_id, observed_at_ms)
                 );
                 CREATE INDEX IF NOT EXISTS subscription_usage_observed ON subscription_usage_history(observed_at_ms);
                 CREATE TABLE IF NOT EXISTS session_model_usage (
                     session_id TEXT NOT NULL, turn_number INTEGER NOT NULL, ordinal INTEGER NOT NULL,
                     agent_id TEXT NOT NULL, role TEXT NOT NULL, provider INTEGER NOT NULL, method INTEGER NOT NULL,
                     model TEXT NOT NULL, input_tokens INTEGER, output_tokens INTEGER,
                     duration_ms INTEGER NOT NULL, observed_at_ms INTEGER NOT NULL,
                     PRIMARY KEY(session_id, turn_number, ordinal),
                     FOREIGN KEY(session_id, turn_number) REFERENCES session_turns(session_id, number) ON DELETE CASCADE
                 );
                 CREATE INDEX IF NOT EXISTS session_model_usage_observed ON session_model_usage(observed_at_ms);
                 PRAGMA user_version = 27;
                 COMMIT;",
            )?;
        }
        if version < 32 {
            connection.execute_batch(
                "BEGIN IMMEDIATE;
                 CREATE TABLE IF NOT EXISTS runbook_counters (
                     workspace_id TEXT PRIMARY KEY,
                     next_id INTEGER NOT NULL DEFAULT 1
                 );
                 CREATE TABLE IF NOT EXISTS runbook_documents (
                     workspace_id TEXT NOT NULL,
                     id TEXT NOT NULL,
                     title TEXT NOT NULL,
                     purpose TEXT NOT NULL,
                     updated_at_ms INTEGER NOT NULL,
                     revision INTEGER NOT NULL,
                     document_json TEXT NOT NULL,
                     PRIMARY KEY(workspace_id,id)
                 );
                 CREATE INDEX IF NOT EXISTS runbook_documents_workspace_updated
                     ON runbook_documents(workspace_id,updated_at_ms DESC,id);
                 CREATE TABLE IF NOT EXISTS runbook_revisions (
                     workspace_id TEXT NOT NULL,
                     runbook_id TEXT NOT NULL,
                     number INTEGER NOT NULL,
                     at_ms INTEGER NOT NULL,
                     actor TEXT NOT NULL,
                     session_id TEXT NOT NULL,
                     reason TEXT NOT NULL,
                     document_json TEXT NOT NULL,
                     PRIMARY KEY(workspace_id,runbook_id,number)
                 );
                 PRAGMA user_version = 32;
                 COMMIT;",
            )?;
        }
        if version < 33 {
            let has_cache_read: bool = connection.query_row(
                "SELECT EXISTS(SELECT 1 FROM pragma_table_info('session_model_usage') WHERE name = 'cache_read_tokens')",
                [],
                |row| row.get(0),
            )?;
            if has_cache_read {
                connection.execute_batch("PRAGMA user_version = 33;")?;
            } else {
                connection.execute_batch(
                    "BEGIN IMMEDIATE;
                     ALTER TABLE session_model_usage ADD COLUMN cache_read_tokens INTEGER;
                     PRAGMA user_version = 33;
                     COMMIT;",
                )?;
            }
        }
        if version < 34 {
            let has_work_elapsed: bool = connection.query_row(
                "SELECT EXISTS(SELECT 1 FROM pragma_table_info('session_model_usage') WHERE name = 'work_elapsed_ms')",
                [],
                |row| row.get(0),
            )?;
            if has_work_elapsed {
                connection.execute_batch("PRAGMA user_version = 34;")?;
            } else {
                connection.execute_batch(
                    "BEGIN IMMEDIATE;
                     ALTER TABLE session_model_usage ADD COLUMN work_elapsed_ms INTEGER;
                     PRAGMA user_version = 34;
                     COMMIT;",
                )?;
            }
        }
        if version < 35 {
            connection.execute_batch(
                "BEGIN IMMEDIATE;
                 CREATE TABLE IF NOT EXISTS runbook_runs (
                     id TEXT PRIMARY KEY,
                     workspace_id TEXT NOT NULL,
                     runbook_id TEXT NOT NULL,
                     session_id TEXT NOT NULL,
                     revision INTEGER NOT NULL,
                     status TEXT NOT NULL CHECK(status IN ('running','completed','failed','interrupted')),
                     output TEXT NOT NULL DEFAULT '',
                     created_at_ms INTEGER NOT NULL,
                     updated_at_ms INTEGER NOT NULL
                 );
                 CREATE INDEX IF NOT EXISTS runbook_runs_session ON runbook_runs(session_id,runbook_id);
                 PRAGMA user_version = 35;
                 COMMIT;",
            )?;
        }
        if version < 36 {
            let tx = connection.transaction()?;
            let present: i64 = tx.query_row(
                "SELECT COUNT(*) FROM pragma_table_info('runbook_runs') WHERE name='current_cell_number'",
                [],
                |row| row.get(0),
            )?;
            if present == 0 {
                tx.execute_batch(
                    "ALTER TABLE runbook_runs ADD COLUMN current_cell_number INTEGER NOT NULL DEFAULT 0;",
                )?;
            }
            tx.execute_batch("PRAGMA user_version = 36;")?;
            tx.commit()?;
        }
        if version < 37 {
            connection.execute_batch(
                "BEGIN IMMEDIATE;
                 CREATE TABLE IF NOT EXISTS runbook_cell_results (
                     workspace_id TEXT NOT NULL,
                     runbook_id TEXT NOT NULL,
                     session_id TEXT NOT NULL,
                     cell_id TEXT NOT NULL,
                     status TEXT NOT NULL CHECK(status IN ('completed','failed')),
                     updated_at_ms INTEGER NOT NULL,
                     PRIMARY KEY(workspace_id,runbook_id,session_id,cell_id)
                 );
                 PRAGMA user_version = 37;
                 COMMIT;",
            )?;
        }
        if version < 38 {
            let tx = connection.transaction()?;
            let present: i64 = tx.query_row(
                "SELECT COUNT(*) FROM pragma_table_info('runbook_runs') WHERE name='current_cell_name'",
                [],
                |row| row.get(0),
            )?;
            if present == 0 {
                tx.execute_batch(
                    "ALTER TABLE runbook_runs ADD COLUMN current_cell_name TEXT NOT NULL DEFAULT '';",
                )?;
            }
            tx.execute_batch("PRAGMA user_version = 38;")?;
            tx.commit()?;
        }
        if version < 39 {
            let tx = connection.transaction()?;
            let present: i64 = tx.query_row(
                "SELECT COUNT(*) FROM pragma_table_info('runbook_cell_results') WHERE name='output'",
                [],
                |row| row.get(0),
            )?;
            if present == 0 {
                tx.execute_batch(
                    "ALTER TABLE runbook_cell_results ADD COLUMN output TEXT NOT NULL DEFAULT '';",
                )?;
            }
            tx.execute_batch("PRAGMA user_version = 39;")?;
            tx.commit()?;
        }
        if version < 40 {
            let tx = connection.transaction()?;
            for (name, definition) in [
                ("exit_code", "INTEGER"),
                ("stdout", "TEXT NOT NULL DEFAULT ''"),
                ("stderr", "TEXT NOT NULL DEFAULT ''"),
            ] {
                let present: i64 = tx.query_row(
                    "SELECT COUNT(*) FROM pragma_table_info('runbook_cell_results') WHERE name=?1",
                    [name],
                    |row| row.get(0),
                )?;
                if present == 0 {
                    tx.execute_batch(&format!(
                        "ALTER TABLE runbook_cell_results ADD COLUMN {name} {definition};"
                    ))?;
                }
            }
            tx.execute_batch("PRAGMA user_version = 40;")?;
            tx.commit()?;
        }
        if version < 41 {
            connection.execute_batch("BEGIN IMMEDIATE;
                 CREATE TABLE IF NOT EXISTS finding_counters(workspace_id TEXT PRIMARY KEY, next_id INTEGER NOT NULL DEFAULT 1);
                 CREATE TABLE IF NOT EXISTS findings(workspace_id TEXT NOT NULL, id TEXT NOT NULL, updated_at_ms INTEGER NOT NULL, revision INTEGER NOT NULL, record_json TEXT NOT NULL, PRIMARY KEY(workspace_id,id));
                 CREATE INDEX IF NOT EXISTS findings_workspace_updated ON findings(workspace_id,updated_at_ms DESC,id);
                 CREATE TABLE IF NOT EXISTS finding_tool_sources(id TEXT PRIMARY KEY, workspace_id TEXT NOT NULL, session_id TEXT NOT NULL, output_sha256 TEXT NOT NULL, created_at_ms INTEGER NOT NULL);
                 PRAGMA user_version = 41;
                 COMMIT;")?;
        }
        if version < 42 {
            findings::migrate_state_structure(&mut connection)?;
        }
        if version < 43 {
            findings::migrate_revisions(&mut connection)?;
        }
        if version < 44 {
            connection.execute_batch("BEGIN IMMEDIATE;
                 CREATE TABLE IF NOT EXISTS session_child_turns (
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
                 CREATE TABLE IF NOT EXISTS memory_turn_outbox (
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
                 CREATE TABLE IF NOT EXISTS session_child_tools (
                     tool_call_id TEXT PRIMARY KEY REFERENCES session_tool_calls(id) ON DELETE CASCADE,
                     agent_id TEXT NOT NULL
                 );
                 CREATE INDEX IF NOT EXISTS memory_turn_outbox_pending ON memory_turn_outbox(status, id);
                 PRAGMA user_version = 44;
                 COMMIT;")?;
        }
        if version < 45 {
            let transaction = connection.transaction()?;
            for (table, sql) in [
                (
                    "session_tool_calls",
                    "ALTER TABLE session_tool_calls ADD COLUMN agent_role TEXT NOT NULL DEFAULT 'main'",
                ),
                (
                    "session_reasoning_summaries",
                    "ALTER TABLE session_reasoning_summaries ADD COLUMN agent_role TEXT NOT NULL DEFAULT 'main'",
                ),
                (
                    "session_commentary",
                    "ALTER TABLE session_commentary ADD COLUMN agent_role TEXT NOT NULL DEFAULT 'main'",
                ),
            ] {
                let exists: bool = transaction.query_row(
                    "SELECT EXISTS(SELECT 1 FROM pragma_table_info(?1) WHERE name = 'agent_role')",
                    [table],
                    |row| row.get(0),
                )?;
                if !exists {
                    transaction.execute_batch(sql)?;
                }
            }
            transaction.execute_batch("PRAGMA user_version = 45")?;
            transaction.commit()?;
        }
        if version < 46 {
            connection.execute_batch("BEGIN IMMEDIATE;
                 CREATE TABLE IF NOT EXISTS session_graph_turns (
                     session_id TEXT NOT NULL REFERENCES sessions(id) ON DELETE CASCADE,
                     turn_number INTEGER NOT NULL,
                     agent_role TEXT NOT NULL,
                     status TEXT NOT NULL CHECK(status IN ('running','completed','failed')),
                     PRIMARY KEY(session_id, turn_number)
                 );
                 CREATE TABLE IF NOT EXISTS session_graph_handoffs (
                     id TEXT PRIMARY KEY,
                     session_id TEXT NOT NULL REFERENCES sessions(id) ON DELETE CASCADE,
                     from_role TEXT NOT NULL,
                     to_role TEXT NOT NULL,
                     task TEXT NOT NULL,
                     status TEXT NOT NULL CHECK(status IN ('pending','running','completed','failed')),
                     created_at_ms INTEGER NOT NULL
                 );
                 CREATE INDEX IF NOT EXISTS session_graph_handoffs_queue ON session_graph_handoffs(session_id,status,to_role,created_at_ms);
                 PRAGMA user_version = 46;
                 COMMIT;")?;
        }
        if version < 47 {
            connection.execute_batch(
                "BEGIN IMMEDIATE;
                 CREATE TABLE IF NOT EXISTS session_graph_provider_context (
                     session_id TEXT NOT NULL REFERENCES sessions(id) ON DELETE CASCADE,
                     agent_role TEXT NOT NULL,
                     context_json TEXT NOT NULL,
                     PRIMARY KEY(session_id, agent_role)
                 );
                 PRAGMA user_version = 47;
                 COMMIT;",
            )?;
        }
        connection.execute(
            "UPDATE runbook_runs SET status='interrupted' WHERE status='running'",
            [],
        )?;
        Ok(Self(Arc::new(Mutex::new(connection))))
    }

    pub fn record_subscription_usage(&self, points: &[SubscriptionUsageRow]) -> Result<()> {
        ensure!(points.len() <= 8, "too many usage windows");
        if points.is_empty() {
            return Ok(());
        }
        self.with_connection(|connection| {
            let transaction = connection.transaction()?;
            for point in points {
                ensure!(
                    point.window_id.len() <= 40
                        && point.label.len() <= 40
                        && (0..=100).contains(&point.used_percent)
                        && point.observed_at_ms >= 0,
                    "invalid subscription history point"
                );
                transaction.execute(
                    "INSERT OR IGNORE INTO subscription_usage_history
                     (provider, window_id, label, observed_at_ms, used_percent, resets_at_unix)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                    params![
                        point.provider,
                        point.window_id,
                        point.label,
                        point.observed_at_ms,
                        point.used_percent,
                        point.resets_at_unix
                    ],
                )?;
            }
            transaction.execute(
                "DELETE FROM subscription_usage_history WHERE observed_at_ms < ?1",
                [points[0].observed_at_ms.saturating_sub(8 * 86_400_000)],
            )?;
            transaction.commit()?;
            Ok(())
        })
    }

    pub fn subscription_history(&self, since_ms: i64) -> Result<Vec<SubscriptionUsageRow>> {
        self.with_connection(|connection| {
            let mut statement = connection.prepare(
                "SELECT provider, window_id, label, observed_at_ms, used_percent, resets_at_unix
                 FROM subscription_usage_history WHERE observed_at_ms >= ?1
                 ORDER BY observed_at_ms, provider, window_id LIMIT 20001",
            )?;
            let rows = statement
                .query_map([since_ms], |row| {
                    Ok(SubscriptionUsageRow {
                        provider: row.get(0)?,
                        window_id: row.get(1)?,
                        label: row.get(2)?,
                        observed_at_ms: row.get(3)?,
                        used_percent: row.get(4)?,
                        resets_at_unix: row.get(5)?,
                    })
                })?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            ensure!(rows.len() <= 20000, "too many subscription history points");
            Ok(rows)
        })
    }

    pub fn model_usage_days(&self, since_ms: i64) -> Result<Vec<ModelUsageDayRow>> {
        self.with_connection(|connection| {
            let mut statement = connection.prepare(
                "SELECT (observed_at_ms / 86400000) * 86400000 AS day, role, provider, method, model,
                        SUM(COALESCE(input_tokens, 0)), SUM(COALESCE(output_tokens, 0)),
                        SUM(duration_ms), SUM(CASE WHEN input_tokens IS NOT NULL AND output_tokens IS NOT NULL THEN 1 ELSE 0 END), COUNT(*),
                        SUM(COALESCE(cache_read_tokens, 0)),
                        SUM(CASE WHEN cache_read_tokens IS NOT NULL THEN COALESCE(input_tokens, 0) ELSE 0 END),
                        COUNT(cache_read_tokens)
                 FROM session_model_usage WHERE observed_at_ms >= ?1
                 GROUP BY day, role, provider, method, model ORDER BY day, provider, model, role LIMIT 2001")?;
            let rows = statement.query_map([since_ms], |row| Ok(ModelUsageDayRow {
                day_start_ms: row.get(0)?, role: row.get(1)?, provider: row.get(2)?,
                method: row.get(3)?, model: row.get(4)?, input_tokens: row.get(5)?,
                output_tokens: row.get(6)?, duration_ms: row.get(7)?,
                measured_calls: row.get(8)?, total_calls: row.get(9)?,
                cache_read_tokens: row.get(10)?, cache_input_tokens: row.get(11)?,
                cache_measured_calls: row.get(12)?,
            }))?.collect::<rusqlite::Result<Vec<_>>>()?;
            ensure!(rows.len() <= 2000, "too many model usage day rows");
            Ok(rows)
        })
    }

    pub fn session_model_usage(&self, session_id: &str) -> Result<Vec<ModelUsageRow>> {
        self.with_connection(|connection| {
            let mut statement = connection.prepare(
                "SELECT agent_id, role, provider, method, model, input_tokens, output_tokens,
                        duration_ms, observed_at_ms, cache_read_tokens, work_elapsed_ms
                 FROM session_model_usage WHERE session_id = ?1
                 ORDER BY turn_number, ordinal LIMIT 2001",
            )?;
            let rows = statement
                .query_map([session_id], |row| {
                    Ok(ModelUsageRow {
                        agent_id: row.get(0)?,
                        role: row.get(1)?,
                        provider: row.get(2)?,
                        method: row.get(3)?,
                        model: row.get(4)?,
                        input_tokens: row.get(5)?,
                        output_tokens: row.get(6)?,
                        duration_ms: row.get(7)?,
                        observed_at_ms: row.get(8)?,
                        cache_read_tokens: row.get(9)?,
                        work_elapsed_ms: row.get(10)?,
                    })
                })?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            ensure!(rows.len() <= 2000, "too many session model usage rows");
            Ok(rows)
        })
    }

    pub fn session_usage_totals(&self, session_id: &str) -> Result<SessionUsageTotals> {
        self.session_usage_totals_for_role(session_id, None)
    }

    pub fn session_usage_totals_for_role(
        &self,
        session_id: &str,
        role: Option<&str>,
    ) -> Result<SessionUsageTotals> {
        self.with_connection(|connection| {
            Ok(connection.query_row(
                "SELECT COALESCE(SUM(COALESCE(input_tokens, 0)), 0),
                        COALESCE(SUM(COALESCE(output_tokens, 0)), 0),
                        COUNT(CASE WHEN input_tokens IS NOT NULL AND output_tokens IS NOT NULL THEN 1 END),
                        COUNT(*),
                        COALESCE(SUM(COALESCE(cache_read_tokens, 0)), 0),
                        COALESCE(SUM(CASE WHEN cache_read_tokens IS NOT NULL THEN input_tokens ELSE 0 END), 0),
                        COUNT(cache_read_tokens)
                 FROM session_model_usage m LEFT JOIN session_graph_turns g
                   ON g.session_id=m.session_id AND g.turn_number=m.turn_number
                 WHERE m.session_id = ?1 AND (?2 IS NULL OR g.agent_role=?2)",
                params![session_id, role],
                |row| Ok(SessionUsageTotals {
                    input_tokens: row.get(0)?,
                    output_tokens: row.get(1)?,
                    measured_calls: row.get(2)?,
                    total_calls: row.get(3)?,
                    cache_read_tokens: row.get(4)?,
                    cache_input_tokens: row.get(5)?,
                    cache_measured_calls: row.get(6)?,
                }),
            )?)
        })
    }

    pub fn session_model_totals(&self, session_id: &str) -> Result<Vec<SessionModelUsageRow>> {
        self.session_model_totals_for_role(session_id, None)
    }

    pub fn session_model_totals_for_role(
        &self,
        session_id: &str,
        role: Option<&str>,
    ) -> Result<Vec<SessionModelUsageRow>> {
        self.with_connection(|connection| {
            let mut statement = connection.prepare(
                "SELECT provider, method, model, role,
                        SUM(COALESCE(input_tokens, 0)), SUM(COALESCE(output_tokens, 0)),
                        COUNT(CASE WHEN input_tokens IS NOT NULL AND output_tokens IS NOT NULL THEN 1 END), COUNT(*),
                        SUM(COALESCE(cache_read_tokens, 0)),
                        SUM(CASE WHEN cache_read_tokens IS NOT NULL THEN COALESCE(input_tokens, 0) ELSE 0 END),
                        COUNT(cache_read_tokens)
                 FROM session_model_usage m LEFT JOIN session_graph_turns g
                   ON g.session_id=m.session_id AND g.turn_number=m.turn_number
                 WHERE m.session_id = ?1 AND (?2 IS NULL OR g.agent_role=?2)
                 GROUP BY provider, method, model, role ORDER BY provider, method, model, role LIMIT 1025",
            )?;
            let rows = statement.query_map(params![session_id, role], |row| Ok(SessionModelUsageRow {
                provider: row.get(0)?, method: row.get(1)?, model: row.get(2)?, role: row.get(3)?,
                totals: SessionUsageTotals {
                    input_tokens: row.get(4)?, output_tokens: row.get(5)?, measured_calls: row.get(6)?,
                    total_calls: row.get(7)?, cache_read_tokens: row.get(8)?, cache_input_tokens: row.get(9)?, cache_measured_calls: row.get(10)?,
                },
            }))?.collect::<rusqlite::Result<Vec<_>>>()?;
            ensure!(rows.len() <= 1024, "too many session model sources");
            Ok(rows)
        })
    }

    pub fn session_usage_snapshots(
        &self,
        session_id: &str,
    ) -> Result<Vec<SessionUsageSnapshotRow>> {
        self.session_usage_snapshots_for_role(session_id, None)
    }

    pub fn session_usage_snapshots_for_role(
        &self,
        session_id: &str,
        role: Option<&str>,
    ) -> Result<Vec<SessionUsageSnapshotRow>> {
        const INTERVAL_MS: u64 = 300_000;
        const MAX_SNAPSHOTS: u64 = 2_048;
        self.with_connection(|connection| {
            let mut statement = connection.prepare(
                "WITH turn_work AS (
                     SELECT number, duration_ms,
                            SUM(duration_ms) OVER (ORDER BY number ROWS UNBOUNDED PRECEDING) - duration_ms AS prior_ms
                     FROM session_turns t LEFT JOIN session_graph_turns g
                       ON g.session_id=t.session_id AND g.turn_number=t.number
                     WHERE t.session_id = ?1 AND (?2 IS NULL OR g.agent_role=?2)
                 )
                 SELECT t.prior_ms, t.duration_ms, m.ordinal, m.input_tokens,
                        m.output_tokens, m.cache_read_tokens, m.work_elapsed_ms, m.role
                 FROM turn_work t LEFT JOIN session_model_usage m
                   ON m.session_id = ?1 AND m.turn_number = t.number
                 ORDER BY t.number, m.ordinal",
            )?;
            let rows = statement.query_map(params![session_id, role], |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, i64>(1)?,
                    row.get::<_, Option<i64>>(2)?,
                    row.get::<_, Option<i64>>(3)?,
                    row.get::<_, Option<i64>>(4)?,
                    row.get::<_, Option<i64>>(5)?,
                    row.get::<_, Option<i64>>(6)?,
                    row.get::<_, Option<String>>(7)?,
                ))
            })?;
            // All three scopes share the same work clock and sampling boundaries.
            let mut changes = BTreeMap::<u64, [SessionUsageTotals; 3]>::new();
            let mut total_work_ms = 0_u64;
            for row in rows {
                let (prior, duration, ordinal, input, output, cached, elapsed, role) = row?;
                let prior = u64::try_from(prior)?;
                let duration = u64::try_from(duration)?;
                total_work_ms = total_work_ms.max(prior.saturating_add(duration));
                if ordinal.is_none() {
                    continue;
                }
                let offset = elapsed.map_or(duration, |value| value.max(0) as u64).min(duration);
                let work_ms = prior.saturating_add(offset);
                let bucket = if work_ms == 0 { 0 } else { (work_ms - 1) / INTERVAL_MS + 1 };
                let mut delta = SessionUsageTotals::default();
                delta.total_calls += 1;
                if let (Some(input), Some(output)) = (input, output) {
                    delta.input_tokens = delta.input_tokens.saturating_add(input);
                    delta.output_tokens = delta.output_tokens.saturating_add(output);
                    delta.measured_calls += 1;
                }
                if let (Some(input), Some(cached)) = (input, cached) {
                    delta.cache_read_tokens = delta.cache_read_tokens.saturating_add(cached);
                    delta.cache_input_tokens = delta.cache_input_tokens.saturating_add(input);
                    delta.cache_measured_calls += 1;
                }
                let scopes = changes.entry(bucket).or_default();
                add_usage_totals(&mut scopes[0], &delta);
                match role.as_deref() {
                    Some(role) if is_top_level_role(role) => add_usage_totals(&mut scopes[1], &delta),
                    Some(role) if !role.is_empty() => add_usage_totals(&mut scopes[2], &delta),
                    _ => {}
                }
            }
            let last_bucket = if total_work_ms == 0 { 0 } else { (total_work_ms - 1) / INTERVAL_MS + 1 };
            let first_bucket = last_bucket.saturating_sub(MAX_SNAPSHOTS - 1);
            let mut totals: [SessionUsageTotals; 3] = Default::default();
            for delta in changes.range(..first_bucket).map(|(_, delta)| delta) {
                for (total, delta) in totals.iter_mut().zip(delta) {
                    add_usage_totals(total, delta);
                }
            }
            let mut snapshots = Vec::with_capacity((last_bucket - first_bucket + 1) as usize);
            for bucket in first_bucket..=last_bucket {
                if let Some(delta) = changes.get(&bucket) {
                    for (total, delta) in totals.iter_mut().zip(delta) {
                        add_usage_totals(total, delta);
                    }
                }
                snapshots.push(SessionUsageSnapshotRow {
                    work_duration_ms: bucket.saturating_mul(INTERVAL_MS).min(total_work_ms),
                    totals: totals[0].clone(),
                    main_totals: totals[1].clone(),
                    subagent_totals: totals[2].clone(),
                });
            }
            Ok(snapshots)
        })
    }

    pub fn session_graph_id(&self, session_id: &str) -> Result<Option<String>> {
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

    pub fn session_graph_config(&self, session_id: &str) -> Result<Option<Vec<u8>>> {
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

    pub fn session_last_top_level_role(&self, session_id: &str) -> Result<Option<String>> {
        self.with_connection(|connection| {
            connection.query_row(
                "SELECT role FROM session_model_usage WHERE session_id = ?1
                 AND role IN ('main','discovery_harness','validation_system','discovery','validation','reporter','patch_generation')
                 ORDER BY turn_number DESC, ordinal DESC LIMIT 1",
                [session_id],
                |row| row.get(0),
            ).optional().map_err(Into::into)
        })
    }

    pub fn session_turn_role(&self, session_id: &str, turn_number: i64) -> Result<Option<String>> {
        self.with_connection(|connection| {
            connection.query_row(
                "SELECT role FROM session_model_usage WHERE session_id = ?1 AND turn_number = ?2
                 AND role IN ('main','discovery_harness','validation_system','discovery','validation','reporter','patch_generation')
                 ORDER BY ordinal DESC LIMIT 1",
                params![session_id, turn_number],
                |row| row.get(0),
            ).optional().map_err(Into::into)
        })
    }

    pub fn provider_context(&self, session_id: &str) -> Result<Option<String>> {
        self.with_connection(|connection| {
            connection
                .query_row(
                    "SELECT context_json FROM session_provider_context WHERE session_id = ?1",
                    [session_id],
                    |row| row.get(0),
                )
                .optional()
                .map_err(Into::into)
        })
    }

    pub fn graph_provider_context(&self, session_id: &str, role: &str) -> Result<Option<String>> {
        self.with_connection(|connection| {
            connection
                .query_row(
                    "SELECT context_json FROM session_graph_provider_context
                     WHERE session_id=?1 AND agent_role=?2",
                    params![session_id, role],
                    |row| row.get(0),
                )
                .optional()
                .map_err(Into::into)
        })
    }

    pub fn automation_jobs(&self) -> Result<Vec<AutomationJobRow>> {
        self.with_connection(|db| {
            let mut query = db.prepare(
                "SELECT id, session_id, workspace_id, title, repeat_preset,
                        next_run_at_ms, last_run_at_ms, canceled
                 FROM automation_jobs ORDER BY canceled, next_run_at_ms, title",
            )?;
            let rows = query.query_map([], |row| {
                Ok(AutomationJobRow {
                    id: row.get(0)?,
                    session_id: row.get(1)?,
                    workspace_id: row.get(2)?,
                    title: row.get(3)?,
                    repeat_preset: row.get(4)?,
                    next_run_at_ms: row.get(5)?,
                    last_run_at_ms: row.get(6)?,
                    canceled: row.get(7)?,
                })
            })?;
            rows.collect::<rusqlite::Result<Vec<_>>>()
                .map_err(Into::into)
        })
    }

    pub fn automation_runs(&self) -> Result<Vec<AutomationRunRow>> {
        self.with_connection(|db| {
            let mut query = db.prepare(
                "SELECT id, job_id, session_id, title, started_at_ms, completed_at_ms, status, error
                 FROM automation_runs ORDER BY status = 1 DESC, started_at_ms DESC LIMIT 1024",
            )?;
            let rows = query.query_map([], |row| {
                Ok(AutomationRunRow {
                    id: row.get(0)?,
                    job_id: row.get(1)?,
                    session_id: row.get(2)?,
                    title: row.get(3)?,
                    started_at_ms: row.get(4)?,
                    completed_at_ms: row.get(5)?,
                    status: row.get(6)?,
                    error: row.get(7)?,
                })
            })?;
            rows.collect::<rusqlite::Result<Vec<_>>>()
                .map_err(Into::into)
        })
    }

    pub fn add_automation_job(&self, session: &SessionRow, preset: &str) -> Result<()> {
        let next = automation_next_run(preset, session.last_message_at_ms, 1)?;
        self.with_connection(|db| {
            let transaction = db.transaction()?;
            transaction.execute(
                "INSERT INTO automation_jobs
                 (id, session_id, workspace_id, title, repeat_preset, next_run_at_ms, last_run_at_ms)
                 VALUES (?1, ?1, ?2, ?3, ?4, ?5, ?6)",
                params![session.id, session.workspace_id, session.title, preset, next, session.last_message_at_ms],
            )?;
            insert_initial_automation_run(&transaction, session, AutomationOptions::default())?;
            transaction.commit()?;
            Ok(())
        })
    }

    pub fn cancel_automation(&self, id: &str, series: bool) -> Result<bool> {
        self.with_connection(|db| {
            let current: Option<(String, i64, i64)> = db
                .query_row(
                    "SELECT a.repeat_preset, o.anchor_at_ms, a.next_occurrence
                     FROM automation_jobs a JOIN automation_job_options o ON o.job_id = a.id
                     WHERE a.id = ?1 AND a.canceled = 0",
                    [id],
                    |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
                )
                .optional()?;
            let Some((preset, anchor, occurrence)) = current else { return Ok(false) };
            if series {
                db.execute("UPDATE automation_jobs SET canceled = 1 WHERE id = ?1", [id])?;
            } else {
                let next_occurrence = occurrence.checked_add(1).context("automation occurrence overflow")?;
                let next = automation_next_run(&preset, anchor, u32::try_from(next_occurrence)?)?;
                db.execute(
                    "UPDATE automation_jobs SET next_run_at_ms = ?2, next_occurrence = ?3 WHERE id = ?1",
                    params![id, next, next_occurrence],
                )?;
            }
            Ok(true)
        })
    }

    pub fn automation_job_canceled(&self, id: &str) -> Result<bool> {
        self.with_connection(|db| {
            db.query_row(
                "SELECT canceled FROM automation_jobs WHERE id = ?1",
                [id],
                |row| row.get(0),
            )
            .map_err(Into::into)
        })
    }

    pub fn claim_due_automation(&self, now_ms: i64) -> Result<Option<AutomationClaim>> {
        self.with_connection(|db| {
            let transaction = db.transaction()?;
            type Due = (String, String, String, i64, i64, String, i32, i32, String, String, i32, bool, i64, i64);
            let due: Option<Due> = transaction.query_row(
                "SELECT a.id, a.workspace_id, a.repeat_preset, a.next_occurrence, o.anchor_at_ms,
                        a.title, t.provider, t.method, t.model, t.prompt,
                        o.reasoning_effort, o.goal_mode, o.goal_max_turns, o.goal_max_duration_minutes
                 FROM automation_jobs a
                 JOIN automation_job_options o ON o.job_id = a.id
                 JOIN session_turns t ON t.session_id = a.session_id AND t.number = 1
                 WHERE a.canceled = 0 AND a.next_run_at_ms <= ?1
                   AND NOT EXISTS (SELECT 1 FROM automation_runs r WHERE r.job_id = a.id AND r.status = 1)
                 ORDER BY a.next_run_at_ms LIMIT 1",
                [now_ms],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?, row.get(4)?,
                    row.get(5)?, row.get(6)?, row.get(7)?, row.get(8)?, row.get(9)?,
                    row.get(10)?, row.get(11)?, row.get(12)?, row.get(13)?)),
            ).optional()?;
            let Some((job_id, workspace_id, preset, occurrence, anchor, title, provider, method,
                model, prompt, effort, goal_mode, max_turns, max_minutes)) = due else { return Ok(None) };
            let next_minimum = u32::try_from(occurrence.checked_add(1).context("automation occurrence overflow")?)?;
            let (next_occurrence, next_due) = next_automation_after(&preset, anchor, next_minimum, now_ms)?;
            let goal_max_turns = u64::try_from(max_turns)?;
            let goal_max_duration_minutes = u64::try_from(max_minutes)?;
            let run_id = uuid::Uuid::new_v4().to_string();
            transaction.execute(
                "UPDATE automation_jobs SET next_occurrence = ?2, next_run_at_ms = ?3 WHERE id = ?1",
                params![job_id, next_occurrence, next_due],
            )?;
            transaction.execute(
                "INSERT INTO automation_runs(id, job_id, title, started_at_ms, status, lease_expires_at_ms)
                 VALUES (?1, ?2, ?3, ?4, 1, ?5)",
                params![run_id, job_id, title, now_ms, now_ms.checked_add(30_000).context("lease overflow")?],
            )?;
            transaction.commit()?;
            Ok(Some(AutomationClaim {
                run_id, job_id, workspace_id, provider, method, model, prompt,
                options: AutomationOptions {
                    reasoning_effort: effort, goal_mode,
                    goal_max_turns,
                    goal_max_duration_minutes,
                },
            }))
        })
    }

    pub fn finish_automation_run(
        &self,
        run_id: &str,
        session_id: Option<&str>,
        status: i32,
        error: &str,
        finished_at_ms: i64,
    ) -> Result<bool> {
        ensure!(matches!(status, 2 | 3), "invalid automation finish status");
        ensure!(error.len() <= 512, "automation error is too long");
        self.with_connection(|db| {
            let transaction = db.transaction()?;
            let changed = transaction.execute(
                "UPDATE automation_runs SET session_id = ?2, completed_at_ms = ?3, status = ?4,
                 error = ?5, lease_expires_at_ms = 0
                 WHERE id = ?1 AND status = 1",
                params![run_id, session_id, finished_at_ms, status, error],
            )?;
            if changed == 1 && status == 2 {
                transaction.execute(
                    "UPDATE automation_jobs SET last_run_at_ms = ?2 WHERE id =
                     (SELECT job_id FROM automation_runs WHERE id = ?1)",
                    params![run_id, finished_at_ms],
                )?;
            }
            transaction.commit()?;
            Ok(changed == 1)
        })
    }

    pub fn interrupt_automation_runs(&self, at_ms: i64) -> Result<usize> {
        self.with_connection(|db| {
            Ok(db.execute(
                "UPDATE automation_runs SET status = 4, completed_at_ms = ?1,
             lease_expires_at_ms = 0,
             error = 'The app-server stopped before this occurrence completed.'
             WHERE status = 1 AND lease_expires_at_ms <= ?1",
                [at_ms],
            )?)
        })
    }

    pub fn renew_automation_lease(&self, run_id: &str, at_ms: i64) -> Result<bool> {
        let expires = at_ms.checked_add(30_000).context("lease overflow")?;
        self.with_connection(|db| {
            Ok(db.execute(
                "UPDATE automation_runs SET lease_expires_at_ms = ?2 WHERE id = ?1 AND status = 1",
                params![run_id, expires],
            )? == 1)
        })
    }

    fn with_connection<T>(&self, action: impl FnOnce(&mut Connection) -> Result<T>) -> Result<T> {
        let mut connection = self
            .0
            .lock()
            .map_err(|_| anyhow::anyhow!("host database lock poisoned"))?;
        action(&mut connection)
    }

    pub fn manual_browsers(&self) -> Result<Vec<ManualBrowserRow>> {
        self.with_connection(|connection| {
            let mut statement = connection
                .prepare("SELECT id, name, executable FROM manual_browsers ORDER BY name, id")?;
            let rows = statement.query_map([], |row| {
                Ok(ManualBrowserRow {
                    id: row.get(0)?,
                    name: row.get(1)?,
                    executable: row.get(2)?,
                })
            })?;
            rows.collect::<rusqlite::Result<Vec<_>>>()
                .map_err(Into::into)
        })
    }

    pub fn add_manual_browser(&self, row: &ManualBrowserRow) -> Result<()> {
        self.with_connection(|connection| {
            connection.execute(
                "INSERT INTO manual_browsers(id, name, executable) VALUES (?1, ?2, ?3)",
                params![row.id, row.name, row.executable],
            )?;
            Ok(())
        })
    }

    pub fn remove_manual_browser(&self, id: &str) -> Result<bool> {
        self.with_connection(|connection| {
            let transaction = connection.transaction()?;
            let removed =
                transaction.execute("DELETE FROM manual_browsers WHERE id = ?1", [id])? > 0;
            if removed {
                transaction.execute(
                    "DELETE FROM fleet_browser_settings WHERE browser_id = ?1",
                    [id],
                )?;
                transaction.execute(
                    "DELETE FROM fleet_preferred_browser WHERE id = 1 AND browser_id = ?1",
                    [id],
                )?;
            }
            transaction.commit()?;
            Ok(removed)
        })
    }

    pub fn browser_settings(&self, id: &str) -> Result<BrowserSettings> {
        self.with_connection(|connection| {
            let (enabled, all_workspaces) = connection
                .query_row(
                    "SELECT enabled, all_workspaces FROM fleet_browser_settings WHERE browser_id = ?1",
                    [id],
                    |row| Ok((row.get(0)?, row.get(1)?)),
                )
                .optional()?
                .unwrap_or((true, true));
            let mut statement = connection.prepare(
                "SELECT workspace_id FROM fleet_browser_workspace_scope WHERE browser_id = ?1 ORDER BY workspace_id",
            )?;
            let ids = statement.query_map([id], |row| row.get(0))?;
            Ok(BrowserSettings {
                enabled,
                all_workspaces,
                workspace_ids: ids.collect::<rusqlite::Result<Vec<_>>>()?,
            })
        })
    }

    pub fn preferred_browser_id(&self) -> Result<Option<String>> {
        self.with_connection(|connection| {
            connection
                .query_row(
                    "SELECT browser_id FROM fleet_preferred_browser WHERE id = 1",
                    [],
                    |row| row.get(0),
                )
                .optional()
                .map_err(Into::into)
        })
    }

    pub fn set_preferred_browser_id(&self, id: &str) -> Result<()> {
        ensure!(
            !id.is_empty() && id.len() <= 80 && !id.chars().any(char::is_control),
            "invalid browser id"
        );
        self.with_connection(|connection| {
            connection.execute(
                "INSERT INTO fleet_preferred_browser(id, browser_id) VALUES (1, ?1)
                 ON CONFLICT(id) DO UPDATE SET browser_id = excluded.browser_id",
                [id],
            )?;
            Ok(())
        })
    }

    pub fn set_browser_settings(&self, id: &str, settings: &BrowserSettings) -> Result<()> {
        ensure!(!id.is_empty() && id.len() <= 80, "invalid browser id");
        ensure!(
            settings.workspace_ids.len() <= 128,
            "too many selected workspaces"
        );
        ensure!(
            !settings.all_workspaces || settings.workspace_ids.is_empty(),
            "all-workspaces scope cannot include workspace ids"
        );
        let mut unique = HashSet::new();
        ensure!(
            settings.workspace_ids.iter().all(|id| {
                !id.is_empty()
                    && id.len() <= 128
                    && !id.chars().any(char::is_control)
                    && unique.insert(id)
            }),
            "invalid or duplicate workspace id"
        );
        self.with_connection(|connection| {
            let transaction = connection.transaction()?;
            transaction.execute(
                "INSERT INTO fleet_browser_settings(browser_id, enabled, all_workspaces) VALUES (?1, ?2, ?3)
                 ON CONFLICT(browser_id) DO UPDATE SET enabled = excluded.enabled, all_workspaces = excluded.all_workspaces",
                params![id, settings.enabled, settings.all_workspaces],
            )?;
            transaction.execute("DELETE FROM fleet_browser_workspace_scope WHERE browser_id = ?1", [id])?;
            for workspace_id in &settings.workspace_ids {
                transaction.execute(
                    "INSERT INTO fleet_browser_workspace_scope(browser_id, workspace_id) VALUES (?1, ?2)",
                    params![id, workspace_id],
                )?;
            }
            transaction.commit()?;
            Ok(())
        })
    }

    pub fn virtual_machine_settings(&self, id: &str) -> Result<VirtualMachineSettings> {
        self.with_connection(|connection| {
            let Some((enabled, all_workspaces, can_run, can_clone)) = connection
                .query_row(
                    "SELECT enabled, all_workspaces, can_run, can_clone FROM fleet_vm_settings WHERE vm_id = ?1",
                    [id],
                    |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
                )
                .optional()?
            else {
                return Ok(VirtualMachineSettings::default());
            };
            let mut statement = connection.prepare(
                "SELECT workspace_id FROM fleet_vm_workspace_scope WHERE vm_id = ?1 ORDER BY workspace_id",
            )?;
            let ids = statement.query_map([id], |row| row.get(0))?;
            Ok(VirtualMachineSettings {
                enabled,
                all_workspaces,
                workspace_ids: ids.collect::<rusqlite::Result<Vec<_>>>()?,
                can_run,
                can_clone,
            })
        })
    }

    pub fn set_virtual_machine_settings(
        &self,
        id: &str,
        settings: &VirtualMachineSettings,
    ) -> Result<()> {
        ensure!(
            !id.is_empty() && id.len() <= 128 && !id.chars().any(char::is_control),
            "invalid VM id"
        );
        ensure!(
            settings.workspace_ids.len() <= 128,
            "too many selected workspaces"
        );
        ensure!(
            !settings.all_workspaces || settings.workspace_ids.is_empty(),
            "all-workspaces scope cannot include workspace ids"
        );
        let mut unique = HashSet::new();
        ensure!(
            settings.workspace_ids.iter().all(|workspace_id| {
                uuid::Uuid::parse_str(workspace_id).is_ok() && unique.insert(workspace_id)
            }),
            "invalid or duplicate workspace id"
        );
        self.with_connection(|connection| {
            let transaction = connection.transaction()?;
            transaction.execute(
                "INSERT INTO fleet_vm_settings(vm_id, enabled, all_workspaces, can_run, can_clone)
                 VALUES (?1, ?2, ?3, ?4, ?5)
                 ON CONFLICT(vm_id) DO UPDATE SET enabled = excluded.enabled,
                     all_workspaces = excluded.all_workspaces,
                     can_run = excluded.can_run, can_clone = excluded.can_clone",
                params![
                    id,
                    settings.enabled,
                    settings.all_workspaces,
                    settings.can_run,
                    settings.can_clone
                ],
            )?;
            transaction.execute(
                "DELETE FROM fleet_vm_workspace_scope WHERE vm_id = ?1",
                [id],
            )?;
            for workspace_id in &settings.workspace_ids {
                transaction.execute(
                    "INSERT INTO fleet_vm_workspace_scope(vm_id, workspace_id) VALUES (?1, ?2)",
                    params![id, workspace_id],
                )?;
            }
            transaction.commit()?;
            Ok(())
        })
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
            transaction.execute(
                "DELETE FROM extension_workspace_scope WHERE workspace_id NOT IN (SELECT id FROM workspaces)",
                [],
            )?;
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

    pub fn create_peer_graph_session(
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
        let automation_next = repeat
            .map(|(preset, _)| automation_next_run(preset, session.last_message_at_ms, 1))
            .transpose()?;
        self.with_connection(|connection| {
            let transaction = connection.transaction()?;
            transaction.execute(
                "INSERT INTO sessions(id,workspace_id,title,created_at_ms,last_message_at_ms)
                 VALUES (?1,?2,?3,?4,?5)",
                params![
                    session.id,
                    session.workspace_id,
                    session.title,
                    session.created_at_ms,
                    session.last_message_at_ms
                ],
            )?;
            transaction.execute(
                "INSERT INTO session_graphs(session_id,graph_id,config_blob) VALUES (?1,?2,?3)",
                params![session.id, graph_id, graph_config],
            )?;
            if let (Some((preset, options)), Some(next)) = (repeat, automation_next) {
                transaction.execute(
                    "INSERT INTO automation_jobs
                     (id,session_id,workspace_id,title,repeat_preset,next_run_at_ms,last_run_at_ms)
                     VALUES (?1,?1,?2,?3,?4,?5,?6)",
                    params![
                        session.id,
                        session.workspace_id,
                        session.title,
                        preset,
                        next,
                        session.last_message_at_ms
                    ],
                )?;
                insert_initial_automation_run(&transaction, session, options)?;
            }
            transaction.commit()?;
            Ok(())
        })
    }

    pub fn is_peer_graph_session(&self, session_id: &str) -> Result<bool> {
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

    pub fn reserve_graph_turn(&self, session_id: &str, role: &str) -> Result<i64> {
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

    pub fn graph_turn_role(&self, session_id: &str, turn_number: i64) -> Result<Option<String>> {
        self.with_connection(|connection| {
            connection.query_row(
                "SELECT agent_role FROM session_graph_turns WHERE session_id=?1 AND turn_number=?2",
                params![session_id,turn_number], |row| row.get(0),
            ).optional().map_err(Into::into)
        })
    }

    pub fn graph_role_status(&self, session_id: &str, role: &str) -> Result<String> {
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

    pub fn finish_graph_turn(
        &self,
        session_id: &str,
        turn_number: i64,
        success: bool,
    ) -> Result<()> {
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

    pub fn enqueue_graph_handoff(
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

    pub fn graph_handoffs(&self, session_id: &str) -> Result<Vec<GraphHandoffRow>> {
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

    pub fn pending_graph_handoffs(&self, session_id: &str) -> Result<Vec<GraphHandoffRow>> {
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

    pub fn set_graph_handoff_status(&self, id: &str, status: &str) -> Result<()> {
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

    pub fn recover_graph_session(&self, session_id: &str) -> Result<()> {
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

    pub fn recover_abandoned_graph_work(&self) -> Result<()> {
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
                "SELECT number, provider, method, model, prompt, answer, duration_ms, internal_prompt
                 FROM (SELECT number, provider, method, model, prompt, answer, duration_ms, internal_prompt
                       FROM session_turns WHERE session_id = ?1 ORDER BY number DESC LIMIT 64)
                 ORDER BY number",
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
                    internal_prompt: row.get(7)?,
                })
            })?;
            Ok(Some(SessionWithTurns {
                session,
                turns: rows.collect::<rusqlite::Result<Vec<_>>>()?,
            }))
        })
    }

    pub fn graph_agent_turns(&self, session_id: &str, role: &str) -> Result<Vec<TurnRow>> {
        self.with_connection(|connection| {
            let mut statement = connection.prepare(
                "SELECT number,provider,method,model,prompt,answer,duration_ms,internal_prompt
                 FROM (SELECT t.number,t.provider,t.method,t.model,t.prompt,t.answer,t.duration_ms,t.internal_prompt
                       FROM session_turns t JOIN session_graph_turns g
                         ON g.session_id=t.session_id AND g.turn_number=t.number
                       WHERE t.session_id=?1 AND g.agent_role=?2
                       ORDER BY t.number DESC LIMIT 64)
                 ORDER BY number",
            )?;
            statement.query_map(params![session_id,role], |row| Ok(TurnRow {
                number: row.get(0)?, provider: row.get(1)?, method: row.get(2)?,
                model: row.get(3)?, prompt: row.get(4)?, answer: row.get(5)?,
                duration_ms: row.get(6)?, internal_prompt: row.get(7)?,
            }))?.collect::<rusqlite::Result<Vec<_>>>().map_err(Into::into)
        })
    }

    pub fn graph_recent_turns(&self, session_id: &str) -> Result<Vec<TurnRow>> {
        self.with_connection(|connection| {
            let mut statement = connection.prepare(
                "SELECT number,provider,method,model,prompt,answer,duration_ms,internal_prompt
                 FROM (SELECT t.number,t.provider,t.method,t.model,t.prompt,t.answer,t.duration_ms,t.internal_prompt,
                         ROW_NUMBER() OVER (PARTITION BY g.agent_role ORDER BY t.number DESC) AS role_ordinal
                       FROM session_turns t JOIN session_graph_turns g
                         ON g.session_id=t.session_id AND g.turn_number=t.number
                       WHERE t.session_id=?1)
                 WHERE role_ordinal<=2 ORDER BY number",
            )?;
            statement
                .query_map([session_id], |row| {
                    Ok(TurnRow {
                        number: row.get(0)?, provider: row.get(1)?, method: row.get(2)?,
                        model: row.get(3)?, prompt: row.get(4)?, answer: row.get(5)?,
                        duration_ms: row.get(6)?, internal_prompt: row.get(7)?,
                    })
                })?
                .collect::<rusqlite::Result<Vec<_>>>()
                .map_err(Into::into)
        })
    }

    pub fn session_total_duration_ms(&self, id: &str) -> Result<u64> {
        self.with_connection(|connection| {
            let total: i64 = connection.query_row(
                "SELECT COALESCE(SUM(duration_ms), 0) FROM session_turns WHERE session_id = ?1",
                [id],
                |row| row.get(0),
            )?;
            u64::try_from(total).map_err(Into::into)
        })
    }

    pub fn session_agent_duration_ms(&self, session_id: &str, role: &str) -> Result<u64> {
        self.with_connection(|connection| {
            let total: i64 = connection.query_row(
                "SELECT COALESCE(SUM(t.duration_ms),0) FROM session_turns t
                 JOIN session_graph_turns g ON g.session_id=t.session_id AND g.turn_number=t.number
                 WHERE t.session_id=?1 AND g.agent_role=?2",
                params![session_id, role],
                |row| row.get(0),
            )?;
            u64::try_from(total).map_err(Into::into)
        })
    }

    pub fn session_has_campaign_activity(&self, id: &str) -> Result<bool> {
        self.with_connection(|connection| {
            connection
                .query_row(
                    "SELECT EXISTS(SELECT 1 FROM runbook_runs WHERE session_id = ?1)
                     OR EXISTS(SELECT 1 FROM session_tool_calls
                 WHERE session_id = ?1 AND (name IN ('graph_delegate','graph_subagents_spawn')
                 OR (name = 'graph_handoff' AND is_error = 0)
                 OR (is_error = 0 AND name IN ('memory_create', 'memory_save', 'lead_create',
                     'finding_create', 'findings_create', 'claim_create', 'runbook_create', 'runbook_edit', 'surface_add', 'surface_save',
                     'surface_mapping_edit', 'surface_promote_element',
                     'surface_threat_model_edit'))))",
                    [id],
                    |row| row.get(0),
                )
                .map_err(Into::into)
        })
    }

    pub fn goal(&self, session_id: &str) -> Result<Option<GoalRow>> {
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

    pub fn feature_enabled(&self, id: &str) -> Result<bool> {
        self.with_connection(|connection| {
            Ok(connection
                .query_row(
                    "SELECT enabled FROM feature_states WHERE id = ?1",
                    [id],
                    |row| row.get::<_, bool>(0),
                )
                .optional()?
                .unwrap_or(false))
        })
    }

    pub fn fleet_totp_secret(&self) -> Result<Option<Vec<u8>>> {
        self.with_connection(|connection| {
            connection
                .query_row(
                    "SELECT secret FROM fleet_totp_secret WHERE id = 1",
                    [],
                    |row| row.get(0),
                )
                .optional()
                .map_err(Into::into)
        })
    }

    pub fn get_or_create_fleet_totp_secret(&self, candidate: &[u8]) -> Result<Vec<u8>> {
        ensure!(candidate.len() == 20, "invalid Fleet TOTP secret length");
        self.with_connection(|connection| {
            let transaction = connection.transaction()?;
            transaction.execute(
                "INSERT OR IGNORE INTO fleet_totp_secret(id, secret) VALUES (1, ?1)",
                [candidate],
            )?;
            let secret: Vec<u8> = transaction.query_row(
                "SELECT secret FROM fleet_totp_secret WHERE id = 1",
                [],
                |row| row.get(0),
            )?;
            transaction.commit()?;
            ensure!(secret.len() == 20, "invalid stored Fleet TOTP secret");
            Ok(secret)
        })
    }

    pub fn extension_scope(&self, kind: &str, id: &str) -> Result<ExtensionWorkspaceScope> {
        ensure!(
            matches!(kind, "plugin" | "feature"),
            "invalid extension kind"
        );
        self.with_connection(|connection| {
            let all_workspaces = connection.query_row(
                "SELECT all_workspaces FROM extension_scope_modes WHERE kind = ?1 AND extension_id = ?2",
                params![kind, id],
                |row| row.get::<_, bool>(0),
            ).optional()?.unwrap_or(true);
            let mut statement = connection.prepare(
                "SELECT workspace_id FROM extension_workspace_scope WHERE kind = ?1 AND extension_id = ?2 ORDER BY workspace_id",
            )?;
            let rows = statement.query_map(params![kind, id], |row| row.get(0))?;
            Ok(ExtensionWorkspaceScope {
                all_workspaces,
                workspace_ids: rows.collect::<rusqlite::Result<Vec<_>>>()?,
            })
        })
    }

    pub fn set_extension_scope(
        &self,
        kind: &str,
        id: &str,
        all_workspaces: bool,
        ids: &[String],
    ) -> Result<()> {
        ensure!(
            matches!(kind, "plugin" | "feature"),
            "invalid extension kind"
        );
        ensure!(!id.is_empty() && id.len() <= 128, "invalid extension id");
        ensure!(ids.len() <= 128, "too many selected workspaces");
        ensure!(
            !all_workspaces || ids.is_empty(),
            "all-workspaces scope cannot include workspace ids"
        );
        let mut unique = HashSet::new();
        ensure!(
            ids.iter().all(|id| !id.is_empty() && unique.insert(id)),
            "duplicate or empty workspace id"
        );
        self.with_connection(|connection| {
            let transaction = connection.transaction()?;
            for workspace_id in ids {
                let exists: bool = transaction.query_row(
                    "SELECT EXISTS(SELECT 1 FROM workspaces WHERE id = ?1)",
                    [workspace_id],
                    |row| row.get(0),
                )?;
                ensure!(exists, "unknown workspace");
            }
            transaction.execute(
                "DELETE FROM extension_workspace_scope WHERE kind = ?1 AND extension_id = ?2",
                params![kind, id],
            )?;
            transaction.execute(
                "INSERT INTO extension_scope_modes(kind, extension_id, all_workspaces) VALUES (?1, ?2, ?3)
                 ON CONFLICT(kind, extension_id) DO UPDATE SET all_workspaces = excluded.all_workspaces",
                params![kind, id, all_workspaces],
            )?;
            for workspace_id in ids {
                transaction.execute(
                    "INSERT INTO extension_workspace_scope(kind, extension_id, workspace_id) VALUES (?1, ?2, ?3)",
                    params![kind, id, workspace_id],
                )?;
            }
            transaction.commit()?;
            Ok(())
        })
    }

    pub fn extension_applies_to(&self, kind: &str, id: &str, workspace_id: &str) -> Result<bool> {
        ensure!(
            matches!(kind, "plugin" | "feature"),
            "invalid extension kind"
        );
        self.with_connection(|connection| {
            connection.query_row(
                "SELECT COALESCE((SELECT all_workspaces FROM extension_scope_modes WHERE kind = ?1 AND extension_id = ?2), 1) = 1
                        OR EXISTS(SELECT 1 FROM extension_workspace_scope WHERE kind = ?1 AND extension_id = ?2 AND workspace_id = ?3)",
                params![kind, id, workspace_id],
                |row| row.get(0),
            ).map_err(Into::into)
        })
    }

    pub fn set_feature_enabled(&self, id: &str, enabled: bool) -> Result<()> {
        self.with_connection(|connection| {
            connection.execute(
                "INSERT INTO feature_states(id, enabled) VALUES (?1, ?2)
                 ON CONFLICT(id) DO UPDATE SET enabled = excluded.enabled",
                params![id, enabled],
            )?;
            Ok(())
        })
    }

    pub fn pause_active_goals(&self, active: i32, paused: i32) -> Result<()> {
        self.with_connection(|connection| {
            connection.execute(
                "UPDATE session_goals SET status = ?1 WHERE status = ?2",
                params![paused, active],
            )?;
            Ok(())
        })
    }

    pub fn pause_active_goals_outside_feature_scope(
        &self,
        feature_id: &str,
        active: i32,
        paused: i32,
    ) -> Result<()> {
        self.with_connection(|connection| {
            connection.execute(
                "UPDATE session_goals SET status = ?1 WHERE status = ?2 AND session_id IN (
                    SELECT sessions.id FROM sessions
                    WHERE sessions.workspace_id NOT IN (
                        SELECT workspace_id FROM extension_workspace_scope
                        WHERE kind = 'feature' AND extension_id = ?3
                    )
                 )",
                params![paused, active, feature_id],
            )?;
            Ok(())
        })
    }

    pub fn set_goal_status(&self, session_id: &str, status: i32) -> Result<()> {
        self.with_connection(|connection| {
            connection.execute(
                "UPDATE session_goals SET status = ?1 WHERE session_id = ?2",
                params![status, session_id],
            )?;
            Ok(())
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
                "INSERT INTO session_turns(session_id, number, provider, method, model, prompt, answer, duration_ms, internal_prompt)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
                params![session.id, turn.number, turn.provider, turn.method, turn.model, turn.prompt, turn.answer, turn.duration_ms, turn.internal_prompt],
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
        self.append_turn_with_goal_activity(
            session,
            turn,
            new_session,
            TurnActivity {
                tools,
                reasoning,
                commentary,
                goal: None,
            },
        )
    }

    pub fn append_turn_with_goal_activity(
        &self,
        session: &SessionRow,
        turn: &TurnRow,
        new_session: bool,
        activity: TurnActivity<'_>,
    ) -> Result<()> {
        self.append_turn_with_goal_and_automation(session, turn, new_session, activity, None)
    }

    pub fn append_turn_with_goal_and_automation(
        &self,
        session: &SessionRow,
        turn: &TurnRow,
        new_session: bool,
        activity: TurnActivity<'_>,
        repeat_preset: Option<(&str, AutomationOptions)>,
    ) -> Result<()> {
        self.append_turn_with_metadata(
            session,
            turn,
            new_session,
            activity,
            repeat_preset,
            TurnMetadata::default(),
        )
    }

    pub fn append_turn_with_metadata(
        &self,
        session: &SessionRow,
        turn: &TurnRow,
        new_session: bool,
        activity: TurnActivity<'_>,
        repeat_preset: Option<(&str, AutomationOptions)>,
        metadata: TurnMetadata<'_>,
    ) -> Result<()> {
        let TurnMetadata {
            provider_context_json,
            graph_id,
            graph_config,
            model_usage,
            child_turns,
            queue_memories,
        } = metadata;
        ensure!(
            provider_context_json.is_none_or(|value| value.len() <= 8_388_608),
            "provider context is too large"
        );
        ensure!(
            repeat_preset.is_none() || new_session,
            "repeat requires a new session"
        );
        ensure!(
            graph_id.is_none_or(|id| new_session && !id.is_empty() && id.len() <= 64),
            "graph requires a new session"
        );
        ensure!(
            graph_config.is_none_or(|value| graph_id.is_some() && value.len() <= 32_768),
            "graph config requires a graph and must be bounded"
        );
        let automation_next = repeat_preset
            .map(|(preset, _)| automation_next_run(preset, session.last_message_at_ms, 1))
            .transpose()?;
        self.with_connection(|connection| {
            let transaction = connection.transaction()?;
            if new_session {
                transaction.execute(
                    "INSERT INTO sessions(id, workspace_id, title, created_at_ms, last_message_at_ms) VALUES (?1, ?2, ?3, ?4, ?5)",
                    params![session.id, session.workspace_id, session.title, session.created_at_ms, session.last_message_at_ms],
                )?;
            }
            if let Some(graph_id) = graph_id {
                transaction.execute(
                    "INSERT INTO session_graphs(session_id, graph_id, config_blob) VALUES (?1, ?2, ?3)",
                    params![session.id, graph_id, graph_config.unwrap_or_default()],
                )?;
            }
            transaction.execute(
                "INSERT INTO session_turns(session_id, number, provider, method, model, prompt, answer, duration_ms, internal_prompt)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
                params![session.id, turn.number, turn.provider, turn.method, turn.model, turn.prompt, turn.answer, turn.duration_ms, turn.internal_prompt],
            )?;
            transaction.execute(
                "UPDATE session_graph_turns SET status='completed'
                 WHERE session_id=?1 AND turn_number=?2 AND status='running'",
                params![session.id, turn.number],
            )?;
            if let Some(model_usage) = model_usage {
                for (ordinal, item) in model_usage.iter().enumerate() {
                    ensure!(!item.agent_id.is_empty() && item.agent_id.len() <= 64
                        && item.model.len() <= 200 && item.role.len() <= 20
                        && item.duration_ms >= 0 && item.observed_at_ms >= 0
                        && item.input_tokens.is_none_or(|value| (0..=1_000_000_000_000).contains(&value))
                        && item.output_tokens.is_none_or(|value| (0..=1_000_000_000_000).contains(&value))
                        && item.cache_read_tokens.is_none_or(|cached| item.input_tokens.is_some_and(|input| (0..=input).contains(&cached)))
                        && item.work_elapsed_ms.is_none_or(|value| (0..=86_400_000).contains(&value)),
                        "invalid model usage");
                    transaction.execute(
                        "INSERT INTO session_model_usage
                         (session_id, turn_number, ordinal, agent_id, role, provider, method, model,
                          input_tokens, output_tokens, duration_ms, observed_at_ms, cache_read_tokens, work_elapsed_ms)
                         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14)",
                        params![session.id, turn.number, ordinal as i64, item.agent_id, item.role,
                            item.provider, item.method, item.model, item.input_tokens,
                            item.output_tokens, item.duration_ms, item.observed_at_ms,
                            item.cache_read_tokens, item.work_elapsed_ms],
                    )?;
                }
            }
            if !new_session {
                transaction.execute(
                    "UPDATE sessions SET last_message_at_ms = MAX(last_message_at_ms, ?2) WHERE id = ?1",
                    params![session.id, session.last_message_at_ms],
                )?;
            }
            let graph_role: Option<String> = transaction.query_row(
                "SELECT agent_role FROM session_graph_turns
                 WHERE session_id=?1 AND turn_number=?2",
                params![session.id, turn.number],
                |row| row.get(0),
            ).optional()?;
            if let Some(context_json) = provider_context_json {
                if let Some(role) = &graph_role {
                    transaction.execute(
                        "INSERT INTO session_graph_provider_context(session_id,agent_role,context_json)
                         VALUES (?1,?2,?3) ON CONFLICT(session_id,agent_role)
                         DO UPDATE SET context_json=excluded.context_json",
                        params![session.id, role, context_json],
                    )?;
                } else {
                    transaction.execute(
                        "INSERT INTO session_provider_context(session_id, context_json) VALUES (?1, ?2)
                         ON CONFLICT(session_id) DO UPDATE SET context_json = excluded.context_json",
                        params![session.id, context_json],
                    )?;
                }
            } else if let Some(role) = graph_role {
                transaction.execute(
                    "DELETE FROM session_graph_provider_context WHERE session_id=?1 AND agent_role=?2",
                    params![session.id, role],
                )?;
            }
            for tool in activity.tools {
                transaction.execute(
                    "INSERT INTO session_tool_calls(id, session_id, turn_number, ordinal, name, summary, arguments, output, is_error, agent_role)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
                    params![tool.id, session.id, turn.number, tool.ordinal, tool.name, tool.summary, tool.arguments, tool.output, tool.is_error, tool.agent_role],
                )?;
            }
            for child in child_turns {
                ensure!(
                    !child.agent_id.is_empty() && child.agent_id.len() <= 64
                        && child.role.len() <= 32 && child.task.len() <= 16_384
                        && child.completed_at_ms >= 0,
                    "invalid child transcript"
                );
                transaction.execute(
                    "INSERT INTO session_child_turns(session_id,parent_turn,agent_id,role,task,result,is_error,messages_json,reasoning_json,completed_at_ms)
                     VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10)",
                    params![session.id, turn.number, child.agent_id, child.role, child.task,
                        child.result, child.is_error, child.messages_json, child.reasoning_json,
                        child.completed_at_ms],
                )?;
                let messages: serde_json::Value = serde_json::from_str(&child.messages_json)?;
                let messages = messages.as_array().context("child messages must be an array")?;
                for message in messages {
                    if message.get("role").and_then(serde_json::Value::as_str) != Some("tool") { continue; }
                    let id = message.get("tool_call_id").and_then(serde_json::Value::as_str)
                        .context("child tool message needs a tool call ID")?;
                    transaction.execute(
                        "INSERT INTO session_child_tools(tool_call_id,agent_id) VALUES (?1,?2)",
                        params![id,child.agent_id],
                    )?;
                }
                if queue_memories {
                    transaction.execute(
                        "INSERT INTO memory_turn_outbox(workspace_id,session_id,turn_number,agent_id) VALUES (?1,?2,?3,?4)",
                        params![session.workspace_id, session.id, turn.number, child.agent_id],
                    )?;
                }
            }
            if queue_memories {
                transaction.execute(
                    "INSERT INTO memory_turn_outbox(workspace_id,session_id,turn_number,agent_id)
                     VALUES (?1,?2,?3,COALESCE((SELECT agent_role FROM session_graph_turns
                       WHERE session_id=?2 AND turn_number=?3),'main'))",
                    params![session.workspace_id, session.id, turn.number],
                )?;
            }
            for item in activity.reasoning {
                transaction.execute(
                    "INSERT INTO session_reasoning_summaries(session_id, turn_number, ordinal, after_tool_count, text, agent_role)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                    params![session.id, turn.number, item.ordinal, item.after_tool_count, item.text, item.agent_role],
                )?;
            }
            for item in activity.commentary {
                transaction.execute(
                    "INSERT INTO session_commentary(session_id, turn_number, ordinal, after_tool_count, text, agent_role)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                    params![session.id, turn.number, item.ordinal, item.after_tool_count, item.text, item.agent_role],
                )?;
            }
            if let Some(goal) = activity.goal {
                ensure!(goal.session_id == session.id, "goal session mismatch");
                transaction.execute(
                    "INSERT INTO session_goals(session_id, objective, status, turns_used, required_state, max_turns, max_duration_minutes, active_ms)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
                     ON CONFLICT(session_id) DO UPDATE SET status = excluded.status,
                         turns_used = excluded.turns_used, required_state = excluded.required_state,
                         active_ms = excluded.active_ms",
                    params![goal.session_id, goal.objective, goal.status, goal.turns_used, goal.required_state, goal.max_turns, goal.max_duration_minutes, goal.active_ms],
                )?;
            }
            if let (Some((preset, options)), Some(next)) = (repeat_preset, automation_next) {
                transaction.execute(
                    "INSERT INTO automation_jobs
                     (id, session_id, workspace_id, title, repeat_preset, next_run_at_ms, last_run_at_ms)
                     VALUES (?1, ?1, ?2, ?3, ?4, ?5, ?6)",
                    params![session.id, session.workspace_id, session.title, preset, next, session.last_message_at_ms],
                )?;
                insert_initial_automation_run(&transaction, session, options)?;
            }
            transaction.commit()?;
            Ok(())
        })
    }

    pub fn tool_summaries(&self, session_id: &str) -> Result<Vec<ToolCallSummaryRow>> {
        self.tool_summaries_since(session_id, 1)
    }

    pub fn tool_summaries_since(
        &self,
        session_id: &str,
        first_turn: i64,
    ) -> Result<Vec<ToolCallSummaryRow>> {
        self.with_connection(|connection| {
            let mut statement = connection.prepare(
                "SELECT id, turn_number, ordinal, name, summary, is_error, agent_role FROM
                 (SELECT id, turn_number, ordinal, name, summary, is_error, agent_role FROM session_tool_calls
                  WHERE session_id = ?1 AND turn_number >= ?2 ORDER BY turn_number DESC, ordinal DESC LIMIT 4097)
                 ORDER BY turn_number, ordinal",
            )?;
            let rows = statement.query_map(params![session_id, first_turn], |row| {
                Ok(ToolCallSummaryRow {
                    id: row.get(0)?,
                    turn_number: row.get(1)?,
                    ordinal: row.get(2)?,
                    name: row.get(3)?,
                    summary: row.get(4)?,
                    is_error: row.get(5)?,
                    agent_role: row.get(6)?,
                })
            })?;
            rows.collect::<rusqlite::Result<Vec<_>>>()
                .map_err(Into::into)
        })
    }

    pub fn tool_summaries_for_turns(
        &self,
        session_id: &str,
        turns: &[i64],
    ) -> Result<Vec<ToolCallSummaryRow>> {
        let turns_json = serde_json::to_string(turns)?;
        self.with_connection(|connection| {
            let mut statement = connection.prepare(
                "SELECT id, turn_number, ordinal, name, summary, is_error, agent_role FROM
                 (SELECT id, turn_number, ordinal, name, summary, is_error, agent_role FROM session_tool_calls
                  WHERE session_id=?1 AND turn_number IN (SELECT value FROM json_each(?2))
                  ORDER BY turn_number DESC, ordinal DESC LIMIT 4097)
                 ORDER BY turn_number, ordinal",
            )?;
            statement
                .query_map(params![session_id, turns_json], |row| {
                    Ok(ToolCallSummaryRow {
                        id: row.get(0)?,
                        turn_number: row.get(1)?,
                        ordinal: row.get(2)?,
                        name: row.get(3)?,
                        summary: row.get(4)?,
                        is_error: row.get(5)?,
                        agent_role: row.get(6)?,
                    })
                })?
                .collect::<rusqlite::Result<Vec<_>>>()
                .map_err(Into::into)
        })
    }

    pub fn reasoning_summaries(&self, session_id: &str) -> Result<Vec<ReasoningSummaryRow>> {
        self.reasoning_summaries_since(session_id, 1)
    }

    pub fn reasoning_summaries_since(
        &self,
        session_id: &str,
        first_turn: i64,
    ) -> Result<Vec<ReasoningSummaryRow>> {
        self.with_connection(|connection| {
            let mut statement = connection.prepare(
                "SELECT turn_number, ordinal, after_tool_count, text, agent_role FROM session_reasoning_summaries
                 WHERE session_id = ?1 AND turn_number >= ?2 ORDER BY turn_number, ordinal LIMIT 8193",
            )?;
            let rows = statement.query_map(params![session_id, first_turn], |row| {
                Ok(ReasoningSummaryRow {
                    turn_number: row.get(0)?,
                    ordinal: row.get(1)?,
                    after_tool_count: row.get(2)?,
                    text: row.get(3)?,
                    agent_role: row.get(4)?,
                })
            })?;
            rows.collect::<rusqlite::Result<Vec<_>>>()
                .map_err(Into::into)
        })
    }

    pub fn reasoning_summaries_for_turns(
        &self,
        session_id: &str,
        turns: &[i64],
    ) -> Result<Vec<ReasoningSummaryRow>> {
        let turns_json = serde_json::to_string(turns)?;
        self.with_connection(|connection| {
            let mut statement = connection.prepare(
                "SELECT turn_number, ordinal, after_tool_count, text, agent_role FROM session_reasoning_summaries
                 WHERE session_id=?1 AND turn_number IN (SELECT value FROM json_each(?2))
                 ORDER BY turn_number, ordinal LIMIT 8193",
            )?;
            statement.query_map(params![session_id, turns_json], |row| Ok(ReasoningSummaryRow {
                turn_number: row.get(0)?, ordinal: row.get(1)?, after_tool_count: row.get(2)?,
                text: row.get(3)?, agent_role: row.get(4)?,
            }))?.collect::<rusqlite::Result<Vec<_>>>().map_err(Into::into)
        })
    }

    pub fn commentary(&self, session_id: &str) -> Result<Vec<CommentaryRow>> {
        self.commentary_since(session_id, 1)
    }

    pub fn commentary_since(
        &self,
        session_id: &str,
        first_turn: i64,
    ) -> Result<Vec<CommentaryRow>> {
        self.with_connection(|connection| {
            let mut statement = connection.prepare(
                "SELECT turn_number, ordinal, after_tool_count, text, agent_role FROM session_commentary
                 WHERE session_id = ?1 AND turn_number >= ?2 ORDER BY turn_number, ordinal LIMIT 8193",
            )?;
            let rows = statement.query_map(params![session_id, first_turn], |row| {
                Ok(CommentaryRow {
                    turn_number: row.get(0)?,
                    ordinal: row.get(1)?,
                    after_tool_count: row.get(2)?,
                    text: row.get(3)?,
                    agent_role: row.get(4)?,
                })
            })?;
            rows.collect::<rusqlite::Result<Vec<_>>>()
                .map_err(Into::into)
        })
    }

    pub fn commentary_for_turns(
        &self,
        session_id: &str,
        turns: &[i64],
    ) -> Result<Vec<CommentaryRow>> {
        let turns_json = serde_json::to_string(turns)?;
        self.with_connection(|connection| {
            let mut statement = connection.prepare(
                "SELECT turn_number, ordinal, after_tool_count, text, agent_role FROM session_commentary
                 WHERE session_id=?1 AND turn_number IN (SELECT value FROM json_each(?2))
                 ORDER BY turn_number, ordinal LIMIT 8193",
            )?;
            statement.query_map(params![session_id, turns_json], |row| Ok(CommentaryRow {
                turn_number: row.get(0)?, ordinal: row.get(1)?, after_tool_count: row.get(2)?,
                text: row.get(3)?, agent_role: row.get(4)?,
            }))?.collect::<rusqlite::Result<Vec<_>>>().map_err(Into::into)
        })
    }

    pub fn child_assistant_messages_for_turns(
        &self,
        session_id: &str,
        turns: &[i64],
    ) -> Result<Vec<ChildAssistantMessageRow>> {
        let turns_json = serde_json::to_string(turns)?;
        self.with_connection(|connection| {
            let mut statement = connection.prepare(
                "SELECT parent_turn, role, text FROM (
                   SELECT c.parent_turn, c.role, json_extract(m.value,'$.text') AS text,
                          c.completed_at_ms AS completed_at, CAST(m.key AS INTEGER) AS message_index
                   FROM session_child_turns c, json_each(c.messages_json) m
                   WHERE c.session_id=?1
                     AND c.parent_turn IN (SELECT value FROM json_each(?2))
                     AND json_extract(m.value,'$.role')='assistant'
                     AND json_type(m.value,'$.text')='text'
                   ORDER BY c.parent_turn DESC, completed_at DESC, message_index DESC LIMIT 256
                 ) ORDER BY parent_turn, completed_at, message_index",
            )?;
            statement
                .query_map(params![session_id, turns_json], |row| {
                    Ok(ChildAssistantMessageRow {
                        parent_turn: row.get(0)?,
                        role: row.get(1)?,
                        text: row.get(2)?,
                    })
                })?
                .collect::<rusqlite::Result<Vec<_>>>()
                .map_err(Into::into)
        })
    }

    pub fn session_children(&self, session_id: &str) -> Result<Vec<SessionChildSummaryRow>> {
        self.with_connection(|connection| {
            let mut statement = connection.prepare(
                "SELECT COALESCE(g.agent_role,'main'),c.agent_id,c.role,c.task,c.is_error,c.completed_at_ms
                 FROM session_child_turns c
                 LEFT JOIN session_graph_turns g ON g.session_id=c.session_id AND g.turn_number=c.parent_turn
                 WHERE c.session_id=?1 ORDER BY c.completed_at_ms DESC,c.agent_id LIMIT 1024",
            )?;
            statement.query_map(params![session_id], |row| Ok(SessionChildSummaryRow {
                parent_role: row.get(0)?, agent_id: row.get(1)?, role: row.get(2)?,
                task: row.get(3)?, is_error: row.get(4)?, completed_at_ms: row.get(5)?,
            }))?
                .collect::<rusqlite::Result<Vec<_>>>().map_err(Into::into)
        })
    }

    pub fn session_child(
        &self,
        session_id: &str,
        agent_id: &str,
    ) -> Result<Option<SessionChildRow>> {
        self.with_connection(|connection| {
            connection.query_row(
                "SELECT COALESCE(g.agent_role,'main'),c.agent_id,c.role,c.task,c.result,
                        c.is_error,c.messages_json,c.reasoning_json,c.completed_at_ms
                 FROM session_child_turns c
                 LEFT JOIN session_graph_turns g ON g.session_id=c.session_id AND g.turn_number=c.parent_turn
                 WHERE c.session_id=?1 AND c.agent_id=?2",
                params![session_id, agent_id], session_child_row,
            ).optional().map_err(Into::into)
        })
    }

    pub fn tool_call(&self, session_id: &str, id: &str) -> Result<Option<ToolCallRow>> {
        self.with_connection(|connection| {
            connection
                .query_row(
                    "SELECT id, turn_number, ordinal, name, summary, arguments, output, is_error, agent_role
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
                            agent_role: row.get(8)?,
                        })
                    },
                )
                .optional()
                .map_err(Into::into)
        })
    }

    pub fn next_memory_job(&self, now_ms: i64) -> Result<Option<MemoryTurnJob>> {
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

    pub fn finish_memory_job(&self, id: i64) -> Result<()> {
        self.with_connection(|connection| {
            connection.execute("DELETE FROM memory_turn_outbox WHERE id=?1", [id])?;
            Ok(())
        })
    }

    pub fn fail_memory_job(&self, id: i64, now_ms: i64) -> Result<()> {
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

    pub fn memory_job_counts(&self, workspace_id: &str) -> Result<(u64, u64)> {
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

    pub fn failed_memory_job_ids(&self, workspace_id: &str) -> Result<Vec<i64>> {
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

    pub fn clear_memory_jobs(&self, workspace_id: &str) -> Result<()> {
        self.with_connection(|connection| {
            connection.execute(
                "DELETE FROM memory_turn_outbox WHERE workspace_id=?1",
                [workspace_id],
            )?;
            Ok(())
        })
    }

    pub fn retry_memory_job(&self, workspace_id: &str, id: i64) -> Result<bool> {
        self.with_connection(|connection| {
            Ok(connection.execute(
                "UPDATE memory_turn_outbox SET status='pending',attempts=0,
                next_attempt_at_ms=0 WHERE workspace_id=?1 AND id=?2 AND status='failed'",
                params![workspace_id, id],
            )? == 1)
        })
    }

    pub fn memory_job_source(&self, job: &MemoryTurnJob) -> Result<String> {
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

    pub fn session_query(
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
                   SELECT 'turn:'||t.number||':user' ref,t.number turn_number,COALESCE(g.agent_role,'main') actor_id,'message' kind,t.prompt body,0 ordinal
                     FROM session_turns t LEFT JOIN session_graph_turns g ON g.session_id=t.session_id AND g.turn_number=t.number WHERE t.session_id=?1
                   UNION ALL SELECT 'turn:'||t.number||':assistant',t.number,COALESCE(g.agent_role,'main'),'message',t.answer,9000 FROM session_turns t LEFT JOIN session_graph_turns g ON g.session_id=t.session_id AND g.turn_number=t.number WHERE t.session_id=?1
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

fn insert_initial_automation_run(
    transaction: &rusqlite::Transaction<'_>,
    session: &SessionRow,
    options: AutomationOptions,
) -> Result<()> {
    transaction.execute(
        "INSERT INTO automation_job_options
         (job_id, reasoning_effort, goal_mode, goal_max_turns, goal_max_duration_minutes, anchor_at_ms)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        params![session.id, options.reasoning_effort, options.goal_mode,
            i64::try_from(options.goal_max_turns)?, i64::try_from(options.goal_max_duration_minutes)?,
            session.last_message_at_ms],
    )?;
    transaction.execute(
        "INSERT INTO automation_runs
         (id, job_id, session_id, title, started_at_ms, completed_at_ms, status)
         VALUES (?1, ?1, ?1, ?2, ?3, ?4, 2)",
        params![
            session.id,
            session.title,
            session.created_at_ms,
            session.last_message_at_ms
        ],
    )?;
    Ok(())
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

    #[test]
    fn long_graph_turn_commits_usage_and_child_transcript_without_work_quota() {
        let root = root();
        let db = HostDatabase::open(&root).unwrap();
        let session = SessionRow {
            id: "session-example".to_owned(),
            workspace_id: "workspace-example".to_owned(),
            title: "Example research".to_owned(),
            created_at_ms: 1,
            last_message_at_ms: 2,
        };
        let usage = (0..513)
            .map(|ordinal| ModelUsageRow {
                agent_id: "scout-example".to_owned(),
                role: "scout".to_owned(),
                provider: 1,
                method: 1,
                model: "example-model".to_owned(),
                input_tokens: Some(100),
                output_tokens: Some(10),
                cache_read_tokens: None,
                work_elapsed_ms: Some(ordinal * 1_000),
                duration_ms: 100,
                observed_at_ms: ordinal * 1_000 + 1,
            })
            .collect::<Vec<_>>();
        let child = ChildTurnRow {
            agent_id: "scout-example".to_owned(),
            role: "scout".to_owned(),
            task: "Inspect a synthetic component".to_owned(),
            result: "x".repeat(65_537),
            is_error: false,
            messages_json: serde_json::json!([{"role":"assistant","text":"x".repeat(1_048_577)}])
                .to_string(),
            reasoning_json: serde_json::json!(["x".repeat(131_073)]).to_string(),
            completed_at_ms: 513_001,
        };
        db.append_turn_with_metadata(
            &session,
            &TurnRow {
                number: 1,
                provider: 1,
                method: 1,
                model: "example-model".to_owned(),
                prompt: "Inspect the example".to_owned(),
                answer: "Example result".to_owned(),
                duration_ms: 513_001,
                internal_prompt: false,
            },
            true,
            TurnActivity {
                tools: &[],
                reasoning: &[],
                commentary: &[],
                goal: None,
            },
            None,
            TurnMetadata {
                model_usage: Some(&usage),
                child_turns: &[child],
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(db.session_model_usage(&session.id).unwrap().len(), 513);
        let saved: (i64, i64, i64) = db.with_connection(|connection| {
            connection.query_row(
                "SELECT length(result),length(messages_json),length(reasoning_json) FROM session_child_turns WHERE session_id=?1",
                [&session.id],
                |row| Ok((row.get(0)?,row.get(1)?,row.get(2)?)),
            ).map_err(Into::into)
        }).unwrap();
        assert!(saved.0 > 65_536 && saved.1 > 1_048_576 && saved.2 > 131_072);
        drop(db);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn tool_summaries_keep_the_latest_calls_of_a_long_turn() {
        let root = root();
        let db = HostDatabase::open(&root).unwrap();
        let session = SessionRow {
            id: "session-example".to_owned(),
            workspace_id: "workspace-example".to_owned(),
            title: "Example session".to_owned(),
            created_at_ms: 1,
            last_message_at_ms: 2,
        };
        let tools = (0..4_100)
            .map(|ordinal| ToolCallRow {
                id: format!("tool-example-{ordinal}"),
                turn_number: 1,
                ordinal,
                name: "read".to_owned(),
                summary: "Read example.txt".to_owned(),
                arguments: "{}".to_owned(),
                output: String::new(),
                is_error: false,
                agent_role: "main".to_owned(),
            })
            .collect::<Vec<_>>();
        db.append_turn_with_metadata(
            &session,
            &TurnRow {
                number: 1,
                provider: 1,
                method: 1,
                model: "example-model".to_owned(),
                prompt: "Example prompt".to_owned(),
                answer: "Example answer".to_owned(),
                duration_ms: 1,
                internal_prompt: false,
            },
            true,
            TurnActivity {
                tools: &tools,
                reasoning: &[],
                commentary: &[],
                goal: None,
            },
            None,
            TurnMetadata::default(),
        )
        .unwrap();
        let summaries = db.tool_summaries_for_turns(&session.id, &[1]).unwrap();
        assert_eq!(summaries.len(), 4_097);
        assert_eq!(summaries.first().unwrap().ordinal, 3);
        assert_eq!(summaries.last().unwrap().ordinal, 4_099);
        drop(db);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn peer_agents_count_as_top_level_usage() {
        for role in [
            "main",
            "discovery_harness",
            "validation_system",
            "discovery",
            "validation",
            "reporter",
            "patch_generation",
        ] {
            assert!(is_top_level_role(role));
        }
        for role in ["scout", "reviewer", "prover", ""] {
            assert!(!is_top_level_role(role));
        }
    }

    fn root() -> std::path::PathBuf {
        std::env::temp_dir().join(format!("beale-storage-example-{}", uuid::Uuid::new_v4()))
    }

    #[test]
    fn new_database_has_no_retired_knowledge_tables() {
        let root = root();
        let db = HostDatabase::open(&root).unwrap();
        let retired: i64 = db
            .with_connection(|connection| {
                Ok(connection.query_row(
                    "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name LIKE 'knowledge_%'",
                    [],
                    |row| row.get(0),
                )?)
            })
            .unwrap();
        assert_eq!(retired, 0);
        drop(db);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn memory_outbox_and_child_transcript_commit_with_turn_and_remain_workspace_scoped() {
        let root = root();
        let db = HostDatabase::open(&root).unwrap();
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
                    goal: None,
                },
                None,
                TurnMetadata {
                    child_turns: &[child],
                    queue_memories: true,
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

    #[test]
    fn tracking_history_persists_and_aggregates_agent_calls() {
        let root = root();
        let db = HostDatabase::open(&root).unwrap();
        let day = 1_800_000_000_000_i64 / 86_400_000 * 86_400_000;
        db.record_subscription_usage(&[SubscriptionUsageRow {
            provider: 1,
            window_id: "weekly".to_owned(),
            label: "Week".to_owned(),
            observed_at_ms: day + 1_000,
            used_percent: 35,
            resets_at_unix: 1_800_100_000,
        }])
        .unwrap();
        let session = SessionRow {
            id: "session-example".to_owned(),
            workspace_id: "workspace-example".to_owned(),
            title: "Example session".to_owned(),
            created_at_ms: day,
            last_message_at_ms: day + 3_000,
        };
        let turn = TurnRow {
            number: 1,
            provider: 1,
            method: 1,
            model: "model-example".to_owned(),
            prompt: "Example prompt".to_owned(),
            answer: "Example answer".to_owned(),
            duration_ms: 2_000,
            internal_prompt: false,
        };
        let usage = [
            ModelUsageRow {
                agent_id: "main".to_owned(),
                role: "main".to_owned(),
                provider: 1,
                method: 1,
                model: "model-example".to_owned(),
                input_tokens: Some(100),
                output_tokens: Some(20),
                cache_read_tokens: Some(37),
                work_elapsed_ms: Some(800),
                duration_ms: 800,
                observed_at_ms: day + 1_000,
            },
            ModelUsageRow {
                agent_id: "scout-example".to_owned(),
                role: "scout".to_owned(),
                provider: 1,
                method: 1,
                model: "model-example".to_owned(),
                input_tokens: None,
                output_tokens: None,
                cache_read_tokens: None,
                work_elapsed_ms: Some(1_500),
                duration_ms: 500,
                observed_at_ms: day + 2_000,
            },
        ];
        db.append_turn_with_metadata(
            &session,
            &turn,
            true,
            TurnActivity {
                tools: &[],
                reasoning: &[],
                commentary: &[],
                goal: None,
            },
            None,
            TurnMetadata {
                model_usage: Some(&usage),
                ..TurnMetadata::default()
            },
        )
        .unwrap();
        drop(db);

        let db = HostDatabase::open(&root).unwrap();
        assert_eq!(db.subscription_history(day).unwrap()[0].used_percent, 35);
        let calls = db.session_model_usage(&session.id).unwrap();
        assert_eq!(
            db.session_last_top_level_role(&session.id)
                .unwrap()
                .as_deref(),
            Some("main")
        );
        assert_eq!(calls.len(), 2);
        assert_eq!(calls[0].agent_id, "main");
        assert_eq!(calls[0].cache_read_tokens, Some(37));
        assert_eq!(calls[1].agent_id, "scout-example");
        assert_eq!(
            db.session_usage_totals(&session.id).unwrap(),
            SessionUsageTotals {
                input_tokens: 100,
                output_tokens: 20,
                measured_calls: 1,
                total_calls: 2,
                cache_read_tokens: 37,
                cache_input_tokens: 100,
                cache_measured_calls: 1,
            }
        );
        let rows = db.model_usage_days(day).unwrap();
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].role, "main");
        assert_eq!((rows[0].input_tokens, rows[0].output_tokens), (100, 20));
        assert_eq!((rows[0].measured_calls, rows[0].total_calls), (1, 1));
        assert_eq!(
            (
                rows[0].cache_read_tokens,
                rows[0].cache_input_tokens,
                rows[0].cache_measured_calls
            ),
            (37, 100, 1)
        );
        assert_eq!(rows[1].role, "scout");
        assert_eq!((rows[1].duration_ms, rows[1].measured_calls), (500, 0));
        assert_eq!(
            (
                rows[1].cache_read_tokens,
                rows[1].cache_input_tokens,
                rows[1].cache_measured_calls
            ),
            (0, 0, 0)
        );
        assert!(db.model_usage_days(day + 86_400_000).unwrap().is_empty());
        let other = SessionRow {
            id: "other-session-example".into(),
            ..session.clone()
        };
        db.append_turn_with_metadata(
            &other,
            &turn,
            true,
            TurnActivity {
                tools: &[],
                reasoning: &[],
                commentary: &[],
                goal: None,
            },
            None,
            TurnMetadata {
                model_usage: Some(&usage),
                ..Default::default()
            },
        )
        .unwrap();
        let sources = db.session_model_totals(&session.id).unwrap();
        assert_eq!(sources.len(), 2);
        assert_eq!(sources[0].role, "main");
        assert_eq!(sources[0].totals.input_tokens, 100);
        assert_eq!(sources[0].totals.cache_read_tokens, 37);
        assert_eq!(sources[0].totals.cache_input_tokens, 100);
        assert_eq!(sources[0].totals.total_calls, 1);
        assert_eq!(sources[1].role, "scout");
        assert_eq!(sources[1].totals.measured_calls, 0);
        assert_eq!(sources[1].totals.total_calls, 1);
        assert_eq!(sources[1].totals.cache_measured_calls, 0);
        let points = db.session_usage_snapshots(&session.id).unwrap();
        let last = points.last().unwrap();
        assert_eq!(last.main_totals.input_tokens, 100);
        assert_eq!(last.main_totals.total_calls, 1);
        assert_eq!(last.subagent_totals.input_tokens, 0);
        assert_eq!(last.subagent_totals.measured_calls, 0);
        assert_eq!(last.subagent_totals.total_calls, 1);
        assert!(
            db.session_model_totals("missing-session-example")
                .unwrap()
                .is_empty()
        );
        let peer_turn = TurnRow {
            number: 2,
            ..turn.clone()
        };
        let peer_usage = [
            ModelUsageRow {
                agent_id: "reporter".to_owned(),
                role: "reporter".to_owned(),
                ..usage[0].clone()
            },
            ModelUsageRow {
                agent_id: "scout-peer-example".to_owned(),
                ..usage[1].clone()
            },
        ];
        db.append_turn_with_metadata(
            &session,
            &peer_turn,
            false,
            TurnActivity {
                tools: &[],
                reasoning: &[],
                commentary: &[],
                goal: None,
            },
            None,
            TurnMetadata {
                model_usage: Some(&peer_usage),
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(
            db.session_last_top_level_role(&session.id)
                .unwrap()
                .as_deref(),
            Some("reporter")
        );
        let peer_last = db
            .session_usage_snapshots(&session.id)
            .unwrap()
            .pop()
            .unwrap();
        assert_eq!(peer_last.main_totals.total_calls, 2);
        assert_eq!(peer_last.subagent_totals.total_calls, 2);
        drop(db);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn usage_snapshots_follow_active_work_and_keep_legacy_calls_at_turn_end() {
        let root = root();
        let db = HostDatabase::open(&root).unwrap();
        let session = SessionRow {
            id: "session-example".to_owned(),
            workspace_id: "workspace-example".to_owned(),
            title: "Example session".to_owned(),
            created_at_ms: 1,
            last_message_at_ms: 2,
        };
        for (number, duration_ms, work_elapsed_ms, input_tokens, cache_read_tokens) in [
            (1, 240_000, Some(180_000), 100, Some(20)),
            (2, 240_000, Some(120_000), 200, None),
            (3, 300_000, None, 50, Some(10)),
        ] {
            let turn = TurnRow {
                number,
                provider: 1,
                method: 1,
                model: "model-example".to_owned(),
                prompt: "Example prompt".to_owned(),
                answer: "Example answer".to_owned(),
                duration_ms,
                internal_prompt: false,
            };
            let usage = [ModelUsageRow {
                agent_id: if number == 2 { "scout-example" } else { "main" }.to_owned(),
                role: if number == 2 { "scout" } else { "main" }.to_owned(),
                provider: 1,
                method: 1,
                model: "model-example".to_owned(),
                input_tokens: Some(input_tokens),
                output_tokens: Some(10),
                cache_read_tokens,
                work_elapsed_ms,
                duration_ms: 100,
                observed_at_ms: number,
            }];
            db.append_turn_with_metadata(
                &session,
                &turn,
                number == 1,
                TurnActivity {
                    tools: &[],
                    reasoning: &[],
                    commentary: &[],
                    goal: None,
                },
                None,
                TurnMetadata {
                    model_usage: Some(&usage),
                    ..TurnMetadata::default()
                },
            )
            .unwrap();
        }
        let snapshots = db.session_usage_snapshots(&session.id).unwrap();
        assert_eq!(
            snapshots
                .iter()
                .map(|item| item.work_duration_ms)
                .collect::<Vec<_>>(),
            [0, 300_000, 600_000, 780_000]
        );
        assert_eq!(
            snapshots
                .iter()
                .map(|item| item.totals.input_tokens)
                .collect::<Vec<_>>(),
            [0, 100, 300, 350]
        );
        assert_eq!(
            snapshots
                .iter()
                .map(|item| item.totals.cache_read_tokens)
                .collect::<Vec<_>>(),
            [0, 20, 20, 30]
        );
        assert_eq!(snapshots.last().unwrap().totals.cache_input_tokens, 150);
        assert_eq!(
            snapshots
                .iter()
                .map(|p| p.main_totals.input_tokens)
                .collect::<Vec<_>>(),
            [0, 100, 100, 150]
        );
        assert_eq!(
            snapshots
                .iter()
                .map(|p| p.subagent_totals.input_tokens)
                .collect::<Vec<_>>(),
            [0, 0, 200, 200]
        );
        let last = snapshots.last().unwrap();
        assert_eq!(last.main_totals.cache_read_tokens, 30);
        assert_eq!(last.main_totals.cache_input_tokens, 150);
        assert_eq!(last.main_totals.total_calls, 2);
        assert_eq!(last.subagent_totals.measured_calls, 1);
        assert_eq!(last.subagent_totals.cache_measured_calls, 0);
        assert_eq!(last.subagent_totals.cache_input_tokens, 0);
        drop(db);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn automation_series_persists_and_skips_one_occurrence() {
        let root = root();
        let db = HostDatabase::open(&root).unwrap();
        let anchor = chrono::DateTime::parse_from_rfc3339("2026-01-31T12:00:00Z")
            .unwrap()
            .timestamp_millis();
        let session = SessionRow {
            id: "session-example".to_owned(),
            workspace_id: "workspace-example".to_owned(),
            title: "Example job".to_owned(),
            created_at_ms: anchor,
            last_message_at_ms: anchor,
        };
        let turn = TurnRow {
            number: 1,
            provider: 1,
            method: 1,
            model: "model-example".to_owned(),
            prompt: "Example prompt".to_owned(),
            answer: "Example answer".to_owned(),
            duration_ms: 1,
            internal_prompt: false,
        };
        db.append_turn_with_goal_and_automation(
            &session,
            &turn,
            true,
            TurnActivity {
                tools: &[],
                reasoning: &[],
                commentary: &[],
                goal: None,
            },
            Some(("month", AutomationOptions::default())),
        )
        .unwrap();
        assert_eq!(db.automation_jobs().unwrap()[0].title, "Example job");
        assert!(!db.automation_job_canceled(&session.id).unwrap());
        assert!(db.cancel_automation(&session.id, false).unwrap());
        assert!(!db.automation_job_canceled(&session.id).unwrap());
        assert_eq!(
            db.automation_jobs().unwrap()[0].next_run_at_ms,
            chrono::DateTime::parse_from_rfc3339("2026-03-31T12:00:00Z")
                .unwrap()
                .timestamp_millis()
        );
        drop(db);
        let reopened = HostDatabase::open(&root).unwrap();
        assert_eq!(reopened.automation_jobs().unwrap().len(), 1);
        assert!(reopened.cancel_automation(&session.id, true).unwrap());
        assert!(reopened.automation_jobs().unwrap()[0].canceled);
        assert!(reopened.automation_job_canceled(&session.id).unwrap());
        assert!(!reopened.cancel_automation(&session.id, true).unwrap());
        drop(reopened);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn due_automation_is_claimed_once_and_recovers_after_lease_expiry() {
        let root = root();
        let db = HostDatabase::open(&root).unwrap();
        let anchor = 1_800_000_000_000_i64;
        let session = SessionRow {
            id: "session-example".to_owned(),
            workspace_id: "workspace-example".to_owned(),
            title: "Example scheduled job".to_owned(),
            created_at_ms: anchor,
            last_message_at_ms: anchor,
        };
        let turn = TurnRow {
            number: 1,
            provider: 1,
            method: 2,
            model: "model-example".to_owned(),
            prompt: "Example prompt".to_owned(),
            answer: "Example answer".to_owned(),
            duration_ms: 1,
            internal_prompt: false,
        };
        db.append_turn_with_goal_and_automation(
            &session,
            &turn,
            true,
            TurnActivity {
                tools: &[],
                reasoning: &[],
                commentary: &[],
                goal: None,
            },
            Some((
                "minute",
                AutomationOptions {
                    reasoning_effort: 3,
                    goal_mode: true,
                    goal_max_turns: 4,
                    goal_max_duration_minutes: 5,
                },
            )),
        )
        .unwrap();
        let due = anchor + 60_000;
        assert!(db.claim_due_automation(due - 1).unwrap().is_none());
        let claim = db.claim_due_automation(due).unwrap().unwrap();
        assert_eq!(claim.prompt, "Example prompt");
        assert_eq!(claim.model, "model-example");
        assert_eq!(claim.options.reasoning_effort, 3);
        assert!(claim.options.goal_mode);
        assert_eq!(claim.options.goal_max_turns, 4);
        assert_eq!(claim.options.goal_max_duration_minutes, 5);
        assert!(db.claim_due_automation(due).unwrap().is_none());
        assert_eq!(db.automation_runs().unwrap()[0].status, 1);
        assert_eq!(db.interrupt_automation_runs(due + 29_999).unwrap(), 0);
        assert!(
            db.renew_automation_lease(&claim.run_id, due + 20_000)
                .unwrap()
        );
        assert_eq!(db.interrupt_automation_runs(due + 40_000).unwrap(), 0);
        assert!(
            db.finish_automation_run(&claim.run_id, Some(&session.id), 2, "", due + 40_000)
                .unwrap()
        );
        assert_eq!(db.automation_runs().unwrap()[0].status, 2);
        assert_eq!(
            db.automation_jobs().unwrap()[0].next_run_at_ms,
            anchor + 120_000
        );
        let next = db.claim_due_automation(anchor + 120_000).unwrap().unwrap();
        assert_eq!(db.interrupt_automation_runs(anchor + 150_000).unwrap(), 1);
        assert_eq!(db.automation_runs().unwrap()[0].status, 4);
        assert!(
            !db.finish_automation_run(&next.run_id, None, 3, "failed", anchor + 150_000)
                .unwrap()
        );
        let late = db.claim_due_automation(anchor + 720_000).unwrap().unwrap();
        assert_eq!(late.prompt, "Example prompt");
        assert_eq!(
            db.automation_jobs().unwrap()[0].next_run_at_ms,
            anchor + 780_000
        );
        assert!(db.claim_due_automation(anchor + 720_000).unwrap().is_none());
        drop(db);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn schema_twenty_three_migrates_existing_automation_jobs() {
        let root = root();
        let db = HostDatabase::open(&root).unwrap();
        let session = SessionRow {
            id: "session-example".to_owned(),
            workspace_id: "workspace-example".to_owned(),
            title: "Example scheduled job".to_owned(),
            created_at_ms: 1_800_000_000_000,
            last_message_at_ms: 1_800_000_000_000,
        };
        db.append_turn(
            &session,
            &TurnRow {
                number: 1,
                provider: 1,
                method: 1,
                model: "model-example".to_owned(),
                prompt: "Example prompt".to_owned(),
                answer: "Example answer".to_owned(),
                duration_ms: 1,
                internal_prompt: false,
            },
            true,
        )
        .unwrap();
        db.add_automation_job(&session, "hour").unwrap();
        db.with_connection(|connection| {
            connection.execute_batch(
                "DROP TABLE automation_runs;
                 DROP TABLE automation_job_options;
                 PRAGMA user_version = 22;",
            )?;
            Ok(())
        })
        .unwrap();
        drop(db);
        let migrated = HostDatabase::open(&root).unwrap();
        assert_eq!(migrated.automation_jobs().unwrap().len(), 1);
        assert_eq!(
            migrated.automation_runs().unwrap()[0].session_id,
            Some(session.id)
        );
        assert_eq!(migrated.automation_runs().unwrap()[0].status, 2);
        let claim = migrated
            .claim_due_automation(1_800_003_600_000)
            .unwrap()
            .unwrap();
        assert_eq!(claim.options, AutomationOptions::default());
        drop(migrated);
        std::fs::remove_dir_all(root).unwrap();
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
            internal_prompt: false,
        };
        db.append_turn_with_metadata(
            &session,
            &first,
            true,
            TurnActivity {
                tools: &[],
                reasoning: &[],
                commentary: &[],
                goal: None,
            },
            None,
            TurnMetadata {
                provider_context_json: None,
                graph_id: Some("simple"),
                graph_config: Some(b"example-config"),
                model_usage: None,
                ..TurnMetadata::default()
            },
        )
        .unwrap();
        assert_eq!(
            db.session_graph_id(&session.id).unwrap().as_deref(),
            Some("simple")
        );
        assert_eq!(
            db.session_graph_config(&session.id).unwrap().as_deref(),
            Some(b"example-config".as_slice())
        );
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
            agent_role: "main".to_owned(),
        };
        let context = "[{\"type\":\"compaction\",\"encrypted_content\":\"opaque-example\"}]";
        db.append_turn_with_metadata(
            &updated_session,
            &second,
            false,
            TurnActivity {
                tools: &[],
                reasoning: std::slice::from_ref(&reasoning),
                commentary: &[],
                goal: None,
            },
            None,
            TurnMetadata {
                provider_context_json: Some(context),
                graph_id: None,
                graph_config: None,
                model_usage: None,
                ..TurnMetadata::default()
            },
        )
        .unwrap();
        assert_eq!(
            db.provider_context(&session.id).unwrap().as_deref(),
            Some(context)
        );
        assert!(
            db.append_turn_with_metadata(
                &updated_session,
                &second,
                false,
                TurnActivity {
                    tools: &[],
                    reasoning: &[],
                    commentary: &[],
                    goal: None,
                },
                None,
                TurnMetadata {
                    provider_context_json: Some("[]"),
                    graph_id: None,
                    graph_config: None,
                    model_usage: None,
                    ..TurnMetadata::default()
                },
            )
            .is_err()
        );
        assert_eq!(
            db.provider_context(&session.id).unwrap().as_deref(),
            Some(context)
        );
        assert_eq!(db.reasoning_summaries(&session.id).unwrap(), [reasoning]);
        assert_eq!(
            db.session(&session.id).unwrap().unwrap().turns,
            [first, second]
        );
        assert_eq!(db.sessions().unwrap(), [updated_session]);
        assert_eq!(db.session_total_duration_ms(&session.id).unwrap(), 2_468);
        assert!(!db.session_has_campaign_activity(&session.id).unwrap());
        assert_eq!(
            db.session_graph_id(&session.id).unwrap().as_deref(),
            Some("simple")
        );
        drop(db);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn session_totals_and_campaign_activity_survive_transcript_trimming() {
        let root = root();
        let db = HostDatabase::open(&root).unwrap();
        let session = SessionRow {
            id: "session-example".to_owned(),
            workspace_id: "workspace-example".to_owned(),
            title: "Example session".to_owned(),
            created_at_ms: 1,
            last_message_at_ms: 1,
        };
        for number in 1..=65 {
            let turn = TurnRow {
                number,
                provider: 1,
                method: 1,
                model: "model-example".to_owned(),
                prompt: "Example prompt".to_owned(),
                answer: "Example answer".to_owned(),
                duration_ms: 1_000,
                internal_prompt: false,
            };
            let tools = (number == 1).then(|| ToolCallRow {
                id: "tool-example".to_owned(),
                turn_number: number,
                ordinal: 0,
                name: "memory_save".to_owned(),
                summary: "Saved example memory".to_owned(),
                arguments: "{}".to_owned(),
                output: "{}".to_owned(),
                is_error: false,
                agent_role: "main".to_owned(),
            });
            let usage = [ModelUsageRow {
                agent_id: "main".into(),
                role: "main".into(),
                provider: 1,
                method: 1,
                model: "model-example".into(),
                input_tokens: Some(100),
                output_tokens: Some(20),
                cache_read_tokens: Some(40),
                work_elapsed_ms: Some(500),
                duration_ms: 500,
                observed_at_ms: number,
            }];
            db.append_turn_with_metadata(
                &session,
                &turn,
                number == 1,
                TurnActivity {
                    tools: tools.as_slice(),
                    reasoning: &[],
                    commentary: &[],
                    goal: None,
                },
                None,
                TurnMetadata {
                    model_usage: Some(&usage),
                    ..Default::default()
                },
            )
            .unwrap();
        }
        assert_eq!(db.session(&session.id).unwrap().unwrap().turns.len(), 64);
        assert_eq!(db.session_total_duration_ms(&session.id).unwrap(), 65_000);
        let models = db.session_model_totals(&session.id).unwrap();
        assert_eq!(models.len(), 1);
        assert_eq!(models[0].totals.total_calls, 65);
        assert_eq!(models[0].totals.input_tokens, 6_500);
        assert_eq!(models[0].totals.cache_read_tokens, 2_600);
        assert!(db.session_has_campaign_activity(&session.id).unwrap());
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
                internal_prompt: false,
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
                internal_prompt: false,
            },
            true,
        )
        .unwrap();
        drop(db);
        let connection = Connection::open(root.join("state.sqlite")).unwrap();
        connection
            .execute_batch(
                "DROP TABLE session_goals;
                 ALTER TABLE session_turns DROP COLUMN internal_prompt;
                 DROP TABLE plugin_installs;
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
            .execute_batch("PRAGMA user_version = 48;")
            .unwrap();
        drop(connection);
        assert!(HostDatabase::open(&root).is_err());
        let connection = Connection::open(root.join("state.sqlite")).unwrap();
        let version: i64 = connection
            .query_row("PRAGMA user_version", [], |row| row.get(0))
            .unwrap();
        assert_eq!(version, 48);
        drop(connection);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn schema_twenty_five_preserves_graph_ids_with_empty_node_settings() {
        let root = root();
        let db = HostDatabase::open(&root).unwrap();
        drop(db);
        let connection = Connection::open(root.join("state.sqlite")).unwrap();
        connection
            .execute_batch(
                "ALTER TABLE session_graphs DROP COLUMN config_blob;
                 INSERT INTO sessions(id, workspace_id, title, created_at_ms, last_message_at_ms)
                 VALUES ('session-example', 'workspace-example', 'Example', 1, 1);
                 INSERT INTO session_graphs(session_id, graph_id)
                 VALUES ('session-example', 'simple');
                 PRAGMA user_version = 25;",
            )
            .unwrap();
        drop(connection);
        let migrated = HostDatabase::open(&root).unwrap();
        assert_eq!(
            migrated
                .session_graph_id("session-example")
                .unwrap()
                .as_deref(),
            Some("simple")
        );
        assert_eq!(
            migrated.session_graph_config("session-example").unwrap(),
            Some(Vec::new())
        );
        drop(migrated);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn schema_nineteen_migrates_browser_settings() {
        let root = root();
        let db = HostDatabase::open(&root).unwrap();
        drop(db);
        let connection = Connection::open(root.join("state.sqlite")).unwrap();
        connection
            .execute_batch(
                "DROP TABLE fleet_browser_workspace_scope;
             DROP TABLE fleet_browser_settings;
             PRAGMA user_version = 18;",
            )
            .unwrap();
        drop(connection);
        let migrated = HostDatabase::open(&root).unwrap();
        let defaults = BrowserSettings {
            enabled: true,
            all_workspaces: true,
            workspace_ids: Vec::new(),
        };
        assert_eq!(
            migrated.browser_settings("detected:example").unwrap(),
            defaults
        );
        let selected = BrowserSettings {
            enabled: false,
            all_workspaces: false,
            workspace_ids: vec![uuid::Uuid::nil().to_string()],
        };
        migrated
            .set_browser_settings("detected:example", &selected)
            .unwrap();
        drop(migrated);
        let reopened = HostDatabase::open(&root).unwrap();
        assert_eq!(
            reopened.browser_settings("detected:example").unwrap(),
            selected
        );
        drop(reopened);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn schema_twenty_migrates_preferred_browser() {
        let root = root();
        let db = HostDatabase::open(&root).unwrap();
        drop(db);
        let connection = Connection::open(root.join("state.sqlite")).unwrap();
        connection
            .execute_batch("DROP TABLE fleet_preferred_browser; PRAGMA user_version = 19;")
            .unwrap();
        drop(connection);
        let migrated = HostDatabase::open(&root).unwrap();
        assert!(migrated.preferred_browser_id().unwrap().is_none());
        migrated
            .set_preferred_browser_id("detected:example")
            .unwrap();
        drop(migrated);
        let reopened = HostDatabase::open(&root).unwrap();
        assert_eq!(
            reopened.preferred_browser_id().unwrap().as_deref(),
            Some("detected:example")
        );
        drop(reopened);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn schema_twenty_one_defaults_vm_allowances_off_and_persists_changes() {
        let root = root();
        let db = HostDatabase::open(&root).unwrap();
        drop(db);
        let connection = Connection::open(root.join("state.sqlite")).unwrap();
        connection
            .execute_batch(
                "DROP TABLE fleet_vm_workspace_scope;
                 DROP TABLE fleet_vm_settings;
                 PRAGMA user_version = 20;",
            )
            .unwrap();
        drop(connection);
        let migrated = HostDatabase::open(&root).unwrap();
        let id = "hyper-v:00000000-0000-4000-8000-000000000001";
        assert_eq!(
            migrated.virtual_machine_settings(id).unwrap(),
            VirtualMachineSettings::default()
        );
        let selected = VirtualMachineSettings {
            enabled: false,
            all_workspaces: false,
            workspace_ids: vec![uuid::Uuid::nil().to_string()],
            can_run: true,
            can_clone: true,
        };
        migrated
            .set_virtual_machine_settings(id, &selected)
            .unwrap();
        drop(migrated);
        let reopened = HostDatabase::open(&root).unwrap();
        assert_eq!(reopened.virtual_machine_settings(id).unwrap(), selected);
        drop(reopened);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn fleet_totp_secret_is_created_once_and_survives_restart() {
        let root = root();
        let db = HostDatabase::open(&root).unwrap();
        assert!(db.fleet_totp_secret().unwrap().is_none());
        assert_eq!(
            db.get_or_create_fleet_totp_secret(&[7; 20]).unwrap(),
            [7; 20]
        );
        assert_eq!(
            db.get_or_create_fleet_totp_secret(&[8; 20]).unwrap(),
            [7; 20]
        );
        drop(db);
        let reopened = HostDatabase::open(&root).unwrap();
        assert_eq!(reopened.fleet_totp_secret().unwrap().unwrap(), [7; 20]);
        drop(reopened);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn schema_thirteen_adds_goal_state_and_internal_prompt_flag() {
        let root = root();
        let db = HostDatabase::open(&root).unwrap();
        let session = SessionRow {
            id: "session-example".to_owned(),
            workspace_id: "workspace-example".to_owned(),
            title: "Example objective".to_owned(),
            created_at_ms: 1,
            last_message_at_ms: 1,
        };
        let turn = TurnRow {
            number: 1,
            provider: 1,
            method: 1,
            model: "model-example".to_owned(),
            prompt: "Example objective".to_owned(),
            answer: "Example observation".to_owned(),
            duration_ms: 1,
            internal_prompt: false,
        };
        db.append_turn(&session, &turn, true).unwrap();
        drop(db);
        let connection = Connection::open(root.join("state.sqlite")).unwrap();
        connection
            .execute_batch(
                "DROP TABLE session_goals;
             ALTER TABLE session_turns DROP COLUMN internal_prompt;
             PRAGMA user_version = 12;",
            )
            .unwrap();
        drop(connection);
        let migrated = HostDatabase::open(&root).unwrap();
        assert!(!migrated.session(&session.id).unwrap().unwrap().turns[0].internal_prompt);
        assert!(migrated.goal(&session.id).unwrap().is_none());
        let goal = GoalRow {
            session_id: session.id.clone(),
            objective: "Example objective".to_owned(),
            status: 1,
            turns_used: 2,
            required_state: String::new(),
            max_turns: 12,
            max_duration_minutes: 45,
            active_ms: 2,
        };
        let continuation = TurnRow {
            number: 2,
            prompt: "Internal continuation".to_owned(),
            internal_prompt: true,
            ..turn
        };
        migrated
            .append_turn_with_goal_activity(
                &session,
                &continuation,
                false,
                TurnActivity {
                    tools: &[],
                    reasoning: &[],
                    commentary: &[],
                    goal: Some(&goal),
                },
            )
            .unwrap();
        assert_eq!(migrated.goal(&session.id).unwrap(), Some(goal));
        assert!(migrated.session(&session.id).unwrap().unwrap().turns[1].internal_prompt);
        drop(migrated);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn schema_fourteen_adds_unlimited_goal_defaults() {
        let root = root();
        let db = HostDatabase::open(&root).unwrap();
        let session = SessionRow {
            id: "session-example".to_owned(),
            workspace_id: "workspace-example".to_owned(),
            title: "Example objective".to_owned(),
            created_at_ms: 1,
            last_message_at_ms: 1,
        };
        let turn = TurnRow {
            number: 1,
            provider: 1,
            method: 1,
            model: "model-example".to_owned(),
            prompt: "Example objective".to_owned(),
            answer: "Example observation".to_owned(),
            duration_ms: 1,
            internal_prompt: false,
        };
        let goal = GoalRow {
            session_id: session.id.clone(),
            objective: turn.prompt.clone(),
            status: 1,
            turns_used: 1,
            required_state: String::new(),
            max_turns: 12,
            max_duration_minutes: 45,
            active_ms: 1,
        };
        db.append_turn_with_goal_activity(
            &session,
            &turn,
            true,
            TurnActivity {
                tools: &[],
                reasoning: &[],
                commentary: &[],
                goal: Some(&goal),
            },
        )
        .unwrap();
        drop(db);
        let connection = Connection::open(root.join("state.sqlite")).unwrap();
        connection
            .execute_batch(
                "ALTER TABLE session_goals DROP COLUMN max_turns;
             ALTER TABLE session_goals DROP COLUMN max_duration_minutes;
             ALTER TABLE session_goals DROP COLUMN active_ms;
             PRAGMA user_version = 13;",
            )
            .unwrap();
        drop(connection);
        let migrated = HostDatabase::open(&root).unwrap();
        assert_eq!(
            migrated.goal(&session.id).unwrap(),
            Some(GoalRow {
                status: 4,
                max_turns: 0,
                max_duration_minutes: 0,
                active_ms: 0,
                ..goal
            })
        );
        drop(migrated);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn schema_fifteen_defaults_features_off_and_persists_enablement() {
        let root = root();
        let db = HostDatabase::open(&root).unwrap();
        drop(db);
        let connection = Connection::open(root.join("state.sqlite")).unwrap();
        connection
            .execute_batch("INSERT INTO sessions(id, workspace_id, title, created_at_ms, last_message_at_ms) VALUES ('session-example', 'workspace-example', 'Example objective', 1, 1);
                INSERT INTO session_goals(session_id, objective, status, turns_used, required_state) VALUES ('session-example', 'Example objective', 1, 1, '');
                DROP TABLE feature_states; PRAGMA user_version = 14;")
            .unwrap();
        drop(connection);
        let db = HostDatabase::open(&root).unwrap();
        assert!(!db.feature_enabled("example-feature").unwrap());
        assert_eq!(db.goal("session-example").unwrap().unwrap().status, 4);
        db.set_feature_enabled("example-feature", true).unwrap();
        drop(db);
        let reopened = HostDatabase::open(&root).unwrap();
        assert!(reopened.feature_enabled("example-feature").unwrap());
        reopened
            .set_feature_enabled("example-feature", false)
            .unwrap();
        assert!(!reopened.feature_enabled("example-feature").unwrap());
        drop(reopened);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn extension_workspace_scope_migrates_and_does_not_broaden_after_deletion() {
        let root = root();
        let db = HostDatabase::open(&root).unwrap();
        drop(db);
        let connection = Connection::open(root.join("state.sqlite")).unwrap();
        connection.execute_batch(
            "DROP TABLE extension_workspace_scope; DROP TABLE extension_scope_modes; PRAGMA user_version = 15;"
        ).unwrap();
        drop(connection);
        let db = HostDatabase::open(&root).unwrap();
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
        assert!(
            db.extension_scope("feature", "goal-mode")
                .unwrap()
                .all_workspaces
        );
        db.set_extension_scope("feature", "goal-mode", false, &["workspace-a".to_owned()])
            .unwrap();
        assert!(
            db.extension_applies_to("feature", "goal-mode", "workspace-a")
                .unwrap()
        );
        assert!(
            !db.extension_applies_to("feature", "goal-mode", "workspace-b")
                .unwrap()
        );
        for workspace_id in ["workspace-a", "workspace-b"] {
            let session = SessionRow {
                id: format!("session-{workspace_id}"),
                workspace_id: workspace_id.to_owned(),
                title: "Example objective".to_owned(),
                created_at_ms: 1,
                last_message_at_ms: 1,
            };
            let goal = GoalRow {
                session_id: session.id.clone(),
                objective: "Example objective".to_owned(),
                status: 1,
                turns_used: 1,
                required_state: String::new(),
                max_turns: 0,
                max_duration_minutes: 0,
                active_ms: 1,
            };
            db.append_turn_with_goal_activity(
                &session,
                &TurnRow {
                    number: 1,
                    provider: 1,
                    method: 1,
                    model: "model-example".to_owned(),
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
                    goal: Some(&goal),
                },
            )
            .unwrap();
        }
        db.pause_active_goals_outside_feature_scope("goal-mode", 1, 4)
            .unwrap();
        assert_eq!(db.goal("session-workspace-a").unwrap().unwrap().status, 1);
        assert_eq!(db.goal("session-workspace-b").unwrap().unwrap().status, 4);
        assert!(
            db.set_extension_scope("feature", "goal-mode", false, &["missing".to_owned()])
                .is_err()
        );
        db.replace_workspaces(&[WorkspaceRow {
            id: "workspace-b".to_owned(),
            name: "Example B".to_owned(),
            directory: "example-b".to_owned(),
        }])
        .unwrap();
        drop(db);
        let reopened = HostDatabase::open(&root).unwrap();
        let scope = reopened.extension_scope("feature", "goal-mode").unwrap();
        assert!(!scope.all_workspaces && scope.workspace_ids.is_empty());
        assert!(
            !reopened
                .extension_applies_to("feature", "goal-mode", "workspace-b")
                .unwrap()
        );
        reopened
            .set_extension_scope("feature", "goal-mode", true, &[])
            .unwrap();
        assert!(
            reopened
                .extension_applies_to("feature", "goal-mode", "workspace-b")
                .unwrap()
        );
        drop(reopened);
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
            .execute_batch("DROP TABLE session_goals; ALTER TABLE session_turns DROP COLUMN internal_prompt; DROP TABLE plugin_installs; ALTER TABLE sessions DROP COLUMN last_message_at_ms; PRAGMA user_version = 10;")
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
                "DROP TABLE session_goals;
                 ALTER TABLE session_turns DROP COLUMN internal_prompt;
                 DROP TABLE plugin_installs;
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
                "DROP TABLE session_goals;
                 ALTER TABLE session_turns DROP COLUMN internal_prompt;
                 DROP TABLE plugin_installs;
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

    #[test]
    fn peer_graph_reservations_and_handoffs_are_durable_and_isolated() {
        let root = root();
        let db = HostDatabase::open(&root).unwrap();
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
                    goal: None,
                },
                None,
                TurnMetadata {
                    provider_context_json: Some(&provider_context),
                    queue_memories: true,
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
        db.set_feature_enabled("beale-memories", true).unwrap();
        assert_eq!(
            db.next_memory_job(200).unwrap().unwrap().agent_id,
            "discovery"
        );
        let transcript = db
            .session_query("workspace-example", "session-example", "", "message", 0, 10)
            .unwrap();
        assert!(transcript.iter().any(|item| item.actor_id == "discovery"));
        assert!(transcript.iter().any(|item| item.actor_id == "validation"));
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
                    goal: None,
                },
                None,
                TurnMetadata::default(),
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
        let db = HostDatabase::open(&root).unwrap();
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
                    goal: None,
                },
                None,
                TurnMetadata {
                    model_usage: Some(&usage),
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

    #[test]
    fn schema_forty_seven_adds_role_context_without_changing_peer_sessions() {
        let root = root();
        let db = HostDatabase::open(&root).unwrap();
        let session = SessionRow {
            id: "session-example".to_owned(),
            workspace_id: "workspace-example".to_owned(),
            title: "Example research".to_owned(),
            created_at_ms: 1,
            last_message_at_ms: 1,
        };
        db.create_peer_graph_session(&session, "openai_pipeline", b"example-config", None)
            .unwrap();
        drop(db);
        let connection = Connection::open(root.join("state.sqlite")).unwrap();
        connection
            .execute_batch("DROP TABLE session_graph_provider_context; PRAGMA user_version = 46;")
            .unwrap();
        drop(connection);
        let migrated = HostDatabase::open(&root).unwrap();
        assert!(migrated.is_peer_graph_session(&session.id).unwrap());
        assert_eq!(
            migrated
                .graph_provider_context(&session.id, "discovery")
                .unwrap(),
            None
        );
        let version: i64 = migrated
            .with_connection(|connection| {
                Ok(connection.query_row("PRAGMA user_version", [], |row| row.get(0))?)
            })
            .unwrap();
        assert_eq!(version, 47);
        drop(migrated);
        std::fs::remove_dir_all(root).unwrap();
    }
}
